# An Incremental Runtime — Design

> Status: design exploration, backed by working prototypes (numbers in §17).
> Audience: someone new to the project. Every term is defined on first use and again in the glossary (§20).

---

## 1. The idea

In a normal stack you *write code → build → deploy → run*. Those are separate worlds with separate tools,
and the seam between them is where slowness and bugs live. This system removes the seam.

**Everything is one incremental computation graph.** Every derived fact — a parsed module, a type, an
optimized bundle, a database query, a rendered table — is a node. "Building" is asking for a node's value
*eagerly, ahead of time*. "Running" is asking for it *lazily, on demand*. They are the same computation
under two scheduling policies, not two systems. An edit is therefore not a rebuild; it is a surgical
update to a few nodes, observable in **microseconds**.

We are building this now because two things have changed. First, **code is increasingly written at
runtime** — by users, increasingly through LLMs — so "ship an artifact ahead of time" is the wrong shape.
Second, **an AI's edit→observe loop must be near-instant**: an agent doing thousands of iterations cannot
pay a seconds-long build cycle each time. A third requirement falls out of the first two: we need
**enormous numbers of concurrent branches** (speculative versions running side by side), which a
copy-per-branch model cannot provide.

### A usage story
Picture the whole system from a user's side.

**You start from nothing.** No cloud, no setup — it runs locally (§14), local-first (§10). And it ships with
**no domain knowledge**: no ontology, no prescribed schema, no model. The model is something you *grow*
bottom-up from the data and from what you and the AI add — the opposite of a platform that hands you a domain
model to conform to first (Cognite's CDM, Palantir's ontology, a DTDL twin all start by prescribing one).

**You drag in whatever data you have.** You fly a drone over your plant and drop the footage in; you upload a
pile of design documents that were lying around; an engineer types in what they know. Each source lands
*through the twin* as a boundary adapter (§9.9) into a layer (§7) — images, video, documents, hand-entered
facts, all pieces of one puzzle. Nothing is converted to a central schema first: each lands in its own shape
and is reconciled by lens (§8.1).

**Things get derived.** Lenses run over what you added (§9) — parsing documents, extracting entities,
computing health from sensor curves — incrementally, only where observed (§3). There is **no single view**:
the value is the graph of derived, connected facts, not one dashboard.

**Connections get drawn — by the system and by you.** Where a rule can tell two things are the same (a tag, a
name, a coordinate), a linking lens draws the edge deterministically and records *why* (§8.1). Where it
can't, **a person draws it** — "this pump in the drone footage is that asset in the manual" — and that
judgment is itself an edit with provenance (§9.7), auditable and replayable. Data quality climbs gradually as
rules and people add connections.

**The AI works alongside you — and on its own.** It fills gaps (extracts a value from a document, infers a
missing field), cleans and reconciles, and proposes connections — and it does so *proactively*: watching the
graph for issues, running scheduled workflows, maintaining a queue of suggestions. Everything it does is an
edit with provenance on a branch (§9.7), so you review and accept rather than trust blindly (§12.4).

**Use-cases are built on top.** "Optimize my staffing," "identify security weaknesses," "what needs
maintenance next" — each is an application: a composition of lenses and views (§11.16) over the *same* graph.
A new use-case is new lenses, not a new system, so each one both consumes the shared data and, by demanding
new derivations, *raises* its quality for the others.

**Everyone browses freely and augments.** Users explore the data (§11.12, virtualized over arbitrarily large
sources), edit through any view (§9.5), and add the connections they happen to know.
Curation is not a separate phase; it is just using the system (§11.10).

**Many people collaborate, git-style.** Everyone can update *locally* without asking — every change is on
their own branch (§7), 0-copy, like a working tree in git. The permission model (§13.1) governs what reaches
*shared* layers (promotion, §7), but local augmentation is always allowed. Branches diff and merge as data
(§11.5).

**It is fully distributed — no cloud, optionally no servers at all.** A peer holds a branch; syncing is
merging branches (§10). For sensitive workloads it runs **peer-to-peer**, with no server in the path — two
machines reconcile directly, echo-suppressed and conflict-resolved per the §18/§19 model. The twin can live
entirely inside an air-gapped plant.

This is the system the rest of the document specifies: *one local-first, distributed graph that anyone can
pour data into, that derives and connects, that any number of use-cases sit on, and that improves the more it
is used.*

### The system at a glance

```
  SURFACES     │  UI host  ·  AI interface  ·  network peer        (each is a boundary lens, §9.3)
   (boundaries)│                    ▲ forward deltas / ▼ backward intents
  ─────────────┼───────────────────────────────────────────────
  BEHAVIOR     │  a DAG of bidirectional lenses                    (§9 — the running application)
   (data graph)│                    ▲ reads (resolved) / ▼ writes (by policy)
  ─────────────┼───────────────────────────────────────────────
  STATE        │  layered store:  ephemeral ▸ branch ▸ user ▸ shared   + event log   (§7, §8)
   (resident)  │
  ─────────────┼───────────────────────────────────────────────
  COMPUTATION  │  content-addressed definitions · incremental engine · governed effects   (§3–§6)
   (code graph)│
  ─────────────┼───────────────────────────────────────────────
  SUBSTRATE    │  V8  (Isolate = one shared heap, Context = one sandbox)   (§14)
```

The rest of this document develops each layer. It is organized so that every capability is a *consequence*
of a small set of commitments, stated next.

---

## 2. Design commitments

The whole system follows from seven decisions. Everything later in this document is one of these
developed, or a consequence of combining them. They are referred to by number (C1…C7) throughout.

- **C1 — One graph.** Every derived fact is a node in a single incremental graph; *build* is eager demand,
  *run* is lazy demand. → no build/run seam; edits cost only their blast radius.
- **C2 — Identity is content.** Code (and cached results) are addressed by the hash of their *normalized
  meaning* plus the hashes of their dependencies. → automatic sharing, exact invalidation, conflict-free
  merge, reproducible references.
- **C3 — Determinism by governance.** A definition's behavior must be a function of `(its code, its
  dependencies, its declared inputs)`. Nondeterminism (clock, randomness, I/O) is *governed* —
  recorded/replayed through injected capabilities — not forbidden. → C2 is *sound*; replay and sandboxing
  become possible.
- **C4 — Pure vs resident.** Nodes are either **pure** (stateless, content-addressed, shareable) or
  **resident** (stateful, per-branch). → cheap branching and clean hot-swap; the dividing line organizes
  the whole system.
- **C5 — Everything is a delta with provenance.** All change flows as *edits* carrying a provenance tag.
  → incrementality, echo suppression, network sync, event sourcing, and lineage are all the same fact.
- **C6 — Behavior is bidirectional lenses.** The running application is a DAG of forward/backward delta
  transforms. Sources, sinks, databases, the DOM, the network, and the AI are all just *boundary lenses*.
  → one uniform model spans data, UI, distribution, and AI.
- **C7 — State is layered.** Resident state is a *resolved stack* of persistent layers, each with its own
  lifecycle, shared structurally. → 0-copy branching, mixed lifecycles (shared/user/branch/ephemeral), and
  unified derivation across all of them.

---

## 3. The incremental engine (C1)

The engine is a spreadsheet taken seriously: every value is a cell depending on other cells; changing an
input recomputes only the cells that depend on it — and only if their *output* actually changed. Two
properties make it fast enough to erase the build/run distinction:

- **Demand-driven.** A node computes only when something downstream asks for it. Unobserved subgraphs cost
  nothing. (This is also what gives virtualization, lazy loading, and partial sync for free later.)
- **Early cutoff.** When a node recomputes to the *same output as before*, propagation stops. A change
  that changes nothing observable costs nothing downstream.

Mechanically, each node carries two stamps — when its value last *changed* and when it was last *verified*
current. Reading a node: if verified this revision, return cache; else check dependencies, and if none
changed since last verify, mark verified and return cache *without recomputing*; else recompute, and bump
the *changed* stamp only if the new output differs. Edit cost is therefore proportional to the **blast
radius** — the nodes whose outputs genuinely change — and nothing more.

> **Validated (A1):** on a 2000-node graph, a typical edit recomputed ~3% of nodes; an edit whose output
> didn't change recomputed **0** downstream.

> **Honest limit.** Some outputs genuinely have a large blast radius — editing a widely-used *type*, or a
> value feeding an **aggregate** over many rows — legitimately change everything downstream; early cutoff
> cannot help when the output really did change. Aggregates split into two classes the engine treats
> differently. **Linear** aggregates (`sum`/`count`/`average`) are cheap: a running value updated per signed
> delta (§9.7), O(groups). **Holistic** aggregates (`min`/`max`/`distinct`/top-k) are *not* self-maintainable
> under deletion — retracting the current extreme forces re-deriving the next, so they retain the whole input
> group, O(rows): a theorem every production incremental engine hits, not a bug to engineer away. Two escape
> hatches: **hierarchical / staged reduction** bounds the *work* to ~log N even when state is N, and
> **append-only inputs** collapse holistic aggregates back to O(groups). The engine classifies every derived
> node as linear (cheap) or holistic (costly) and surfaces that in its cost model — "it is all one graph" must
> not hide that some nodes are orders of magnitude costlier. This is the engine's one real algorithmic limit,
> named up front.

This is established theory (Salsa, Adapton, differential dataflow, Jane Street `incremental`); the novelty
is applying it to *everything*, not one compiler pass.

---

## 4. Content-addressed definitions (C2)

A **definition** is the unit of code — one function or value. Its **identity is the hash of its normalized
syntax tree plus the hashes of its dependencies.** Normalization strips formatting, comments, and local
variable names, so two definitions that *mean* the same thing have the same hash. References are by hash,
not by name; **names live in a separate table — a namespace.** (This is Unison's model and Git's object
store, applied to live code.)

Everything good here is a direct consequence:

- **Renaming is free** and never breaks callers (callers reference a hash, not a name).
- **Identical code is shared automatically** — two branches with the same function point at one object.
- **Invalidation is exact** — an edit makes a *new* definition (new hash) and rebinds a name; the engine
  already knows which nodes depended on the old hash (C1).
- **Merge is conflict-free by construction** — two branches that change *different* definitions never
  conflict; only edits to the *same* definition can.

This is why version control is *our own*, at definition granularity (§15) rather than git-over-files.

### 4.1 Lenses are data: the code graph lives in the data graph
A lens's **configuration *and* its implementation are themselves data in the twin** — content-addressed
definitions (this section), viewed and edited through the *same* lens system that handles everything else.
This is the reflective heart of the design; it sounds meta, but it is what makes the system uniform.

- **A lens definition is a source like any other.** Its implementation is a normalized AST (C2) and its
  configuration is JSON (§9.15); both are content-addressed data. So you view, diff, and edit a lens *through
  a lens* — a code editor or a config form is a component-lens (§11.6, §11.15) over the definition-as-data,
  exactly as a table is a component-lens over rows.
- **It is governed identically to data.** Editing a lens is an edit-with-provenance on a branch (§8 — code
  and data edits are the *same* event); it is permissioned per-edit (§13.1), lineage-tracked (§8), reviewed
  and promoted (§7), branched 0-copy (§7), and deletable / GC'd (§8.2). There is no separate "code management"
  system, because code is data.
- **It both *is* data and *configures* the running graph.** A definition is at once a piece of
  content-addressed data (editable, governable) and the configuration of a resident node (§5); editing it as
  data triggers a hot-swap (§5.1) of the running node. The two-graph framing (§5) is one view; "the code graph
  is a region of the data graph" is the other — both hold at once.
- **You can build lenses over lenses.** Because code is data, a lens can derive *from* the code graph: list
  every lens touching an asset, derive the dependency graph, flag un-governed effects (§6), diff two branches'
  models. The AI's projections (§12.3) are exactly this — lenses over the code graph — and the AI authoring a
  lens (§11.10) or proposing one (§12.4) is just editing data through lenses, governed like any proposal.

**Where the tower bottoms out (the honest meta-answer).** Self-reference does not regress infinitely. A
**small, fixed, audited kernel** is *not* lens-governed: the incremental engine (§3), the content-addressed
store and name resolution (§4, §7), the governed-effect boundary (§6), and the bare-V8 substrate (§14). Above
that kernel, *everything* — lenses, schemas, applications, and even the lenses that view and edit lenses — is
data governed by lenses. The reflective tower stands on a trusted base, the way a metacircular interpreter
bottoms out at its host or git's porcelain bottoms out at its plumbing. That base is deliberately tiny,
because a smaller trusted kernel is the §13 security argument restated: the less that is privileged, the less
there is to get wrong.

---

## 5. Pure and resident nodes (C4)

C4 is the load-bearing distinction, so it gets its own short section. Every node is one of:

- **Pure** — content-addressed (C2), immutable, shareable *by reference* across branches. Parsing,
  type-checking, the output of a function over pure inputs, a lens's *code*.
- **Resident** — identity over time, holds mutable state: the DOM, a socket, a timer, a component's local
  state, a lens *instance's* materialized value and provenance, a row in the data store.

The mantra is **shared pure brain, separate stateful body.** Branching shares all pure structure for free
and forks only resident state (§7). Hot-swapping replaces pure definitions while resident state persists
or migrates. The two-graphs framing used throughout — a **code graph** (pure, branchable) that
*configures* a **data graph** (resident, running) — is just C4 viewed at system scale.

### 5.1 Hot-swap and live state migration
Editing a lens makes a *new* definition (new hash, §4) while resident instances still hold state shaped by the
old one. "Persists or migrates" resolves into three cases by state-compatibility:

- **State-compatible (the common case).** The edit changes behavior but not the *shape* of resident state (a
  reformulated `forward`, a new threshold). The new definition takes over and resident state persists
  untouched — this is the §14 rebind→observe path (42 µs). No migration.
- **Shape change.** The new state has a different shape (added field, changed representation). This needs a
  **migration function** `migrate: oldState → newState`, *itself a content-addressed, governed, testable
  definition*. The author or an LLM writes it; the engine auto-derives trivial migrations (add nullable field,
  rename) and **rejects non-trivial ones at edit time** rather than silently corrupting state.
- **Incompatible / no migration.** Where the node is a *pure-derived materialization*, just **rebuild it from
  inputs** (§3) — no migration needed. Where it is a true state source, either replay event-sourced state
  through a migration or reset to initial under explicit policy (never blind-replay old events through new
  code, which §8 warns against).

Three properties make this safe rather than reckless: migrations are **versioned, lineage-tracked edits** (§8),
so a branch's state-shape history is reproducible and reversible; they run **on a branch first** (§7), diffed
and reviewed (§11.5) before promotion — never in-place on shared state; and they are a **live-mode** operation
(§14), accepting the de-opt tax that ship/freeze mode avoids.

> **Honest limit.** General shape-changing migration is unsolved in the abstract — it is database schema
> migration, and semantic preservation cannot be guaranteed automatically. We make it *explicit, tested,
> branch-isolated, and reversible* instead of automatic. A migration over large resident state costs
> proportional to that state (not incremental) — a genuine, bounded blast radius.

---

## 6. Determinism by governed effects (C3)

C2 assumes a definition's behavior is fixed by `(code, deps)`. Plain JavaScript breaks that constantly:
`Math.random()`, `Date.now()`, file and network reads make the *same code* behave differently. Unchecked,
two branches could share a cached result that is actually wrong, and replaying history (§8) would produce
a different past. C3 is the fix, and it is the linchpin the rest of the system leans on.

We do not forbid nondeterminism (that would force the AI into an unfamiliar dialect). We **govern** it. Per
context, the engine swaps the nondeterministic primitives for governed versions that either **record**
each value they produce or **replay** values from a log, then **freezes** them so code cannot reassign
them to escape. A definition never imports I/O; it receives **injected capabilities** (a `ctx`), the only
governed door to the outside.

> **Validated (effects):** an idiomatic lens using `Math.random()`/`Date.now()` produced byte-identical
> output across two branches with the same seed (sharing is *sound*); its recorded log alone reproduced
> its state exactly (replay is *complete*); an ungoverned version diverged (showing why governance is
> required); attempts to reassign the frozen primitive failed; a lens reaching an un-governed source was
> statically rejected.

One mechanism, three payoffs at once: **sound hashing** (so C2's sharing is correct), **deterministic
replay** (so C5/§8's event sourcing works), and **safe sandboxing** (so §13's capability model is the real
boundary). Bare V8 helps: with no Node and no DOM globals, the nondeterminism set is small and enumerable — its
completeness, the three dispositions, and the cross-machine-float caveat are developed in §6.1.

### 6.1 The governed set: completeness
C2's caching soundness and §13's sandbox both require that *every* nondeterminism / effect source in the
context is governed — one miss is either a wrong cached result or a sandbox escape. Bare V8 (no Node, no DOM)
makes the set **finite and enumerable**, and each source gets one of three dispositions:

- **Delete** it from the context (no ambient authority) — the strongest enforcement; a source not present
  cannot leak (un-governed `fetch`, un-injected I/O).
- **Replace + freeze** with a recording/replaying version — time (`Date.now`, `new Date`, `performance.now`),
  entropy (`Math.random`, `crypto.getRandomValues`). Freezing blocks reassignment (validated, §6).
- **Canonicalize** where determinism is reachable — fixed timezone/locale (`Intl`), stable iteration order,
  normalized error surfaces; GC-observable `WeakRef`/`FinalizationRegistry` and microtask/timer scheduling
  are governed or removed.

Because **per-Context globals are ours to define** (§14), governance is "construct the allowed context," not
"patch a shared one." Two mechanisms make completeness real rather than aspirational: a lens reaching an
un-governed source is **statically rejected at authoring** (§9.4/§9.10, validated), enforcing at the gate;
and a **conformance suite** — a maintained battery asserting each enumerated source is governed/deleted — runs
against every V8 version, because the set *drifts* across versions. Completeness is therefore a **maintained
property guarded by tests**, not a one-time proof.

> **Honest limits.** **Cross-machine floating-point determinism** (transcendental precision across CPUs/libms)
> is genuinely hard and deferred — same-machine replay is exact. **Timing / Spectre-class side-channels** are
> not closed by governance and need process isolation (§13). The set is finite but **version-fragile** — a new
> V8 primitive can introduce a source; the conformance suite is how we catch it.

---

## 7. Layered state and branches (C7)

Resident state (C4) is not a flat store. It is a **stack of layers**, resolved top-down, each layer an
independent persistent map with its own lifecycle:

| Layer (top→bottom) | Scope | Lifecycle |
|---|---|---|
| **ephemeral** | one interaction / tab | scratch; dropped on reload unless promoted |
| **branch** | one speculative branch | cheap divergence; discarded or merged |
| **user** | one user, all their branches | event-sourced + checkpointed |
| **shared** | everyone | durable, content-addressed, versioned |

Three properties, all measured:

- **Resolution is a top-down walk.** A read returns the first layer that has the key; a **tombstone** in a
  higher layer hides the key in all lower ones (so a branch can locally "delete" a shared row). This is the
  union-overlay idea (OverlayFS, Docker layers, the CSS cascade, git's index) at object granularity.
- **Branching is 0-copy.** Each layer is a persistent, structurally-shared map; a branch is just two empty
  top layers (branch + ephemeral) over *pointers* to the shared lower layers. Forking copies pointers;
  divergence costs only what is written.
- **Lifecycle is per-layer and independent.** Dropping all ephemeral state is resetting one pointer per
  branch; GC reclaims only ephemeral nodes. Each layer checkpoints, persists, and GCs on its own schedule.

> **Validated (overlay, layers):** one million single-layer branches at **~2.8 KB each, 0.7 µs to fork**
> (vs ~90 TB for copy-per-branch). With a four-layer stack, 200k branches over a shared 686k-node base
> diverged by ~12 nodes (~1.7 KB) each at 0.25 µs; top-down resolution with tombstones and wholesale
> ephemeral-drop both behaved as specified.

The crucial consequence: because resolution produces *one merged view*, a derivation (§9) reads the
**resolved value and never knows which layer it came from.** A view can join shared catalog data + a
user's saved cart + the quantity *currently being typed* (ephemeral) as if they were one table. That is
what "different state has different lifecycle, yet is uniformly derivable" means concretely.

**Promotion** moves data between layers — "save draft" (ephemeral→user), "publish" (user→shared), "merge
branch" — as explicit, lineage-tracked operations, never copies. And because each lower layer is an
immutable version, every branch sees a consistent snapshot of it: this is **MVCC snapshot isolation**, so
concurrent branches never see torn reads. *Which* layer a write lands in is exactly the node's
**persistence policy** — a configuration on the node, not a property of the lens that wrote it. The same
component can be wired so a field is a local scratch value, an event-sourced durable record, or a
write-through to an external system, by changing one policy (§11.8).

---

## 8. History: event sourcing, lineage, recovery (C5)

Because every change is a delta with provenance (C5), the **durable, append-only log of edits is the
source of truth**, and live state is a *materialized view* of it — disposable and rebuildable. Code edits
and data edits are the same kind of event. This gives crash recovery, time-travel, and debugging-by-replay
for free, and it pairs with §7: the persistent store yields **free snapshots** (every branch root *is* a
snapshot), so recovery replays the log only from the nearest snapshot, not from genesis. Because code is
content-addressed (C2), each event records the **exact code version that processed it**, so replay uses
historical code — no "new code misreads old events." Replay rebuilds *internal* state only; external
side-effects are confined to boundary adapters (§9.9), where replay stops, so recovery never double-writes.

**Lineage has two axes, and the system gives both:**

- **Edit lineage** ("who/when/what") — the event log itself: every edit, tagged, ordered, replayable to
  any point.
- **Derivation lineage** ("what produced this value") — the dependency edges the engine already records
  (C1): any derived value traces back through its inputs and the exact lens versions (C2) and recorded
  effect-log entries (C3) that produced it.

Combined, you can answer the complete question — *this value came from these source rows, through these
lens versions, producing this recorded LLM output, at this time, caused by this edit.* Provenance is
**intrinsic**, not bolted on, because content-addressing + dependency tracking + the log already carry it.
The cost is storage growth, addressed by snapshot+log truncation and coarse-graining old lineage.

### 8.1 Integration is deterministic linking + provenance, not probabilistic matching
The hardest part of an industrial twin is *integration*: fusing many heterogeneous sources (WITSML, EDM,
sensor histories, reports) into one coherent model. The incumbent answer is a fuzzy entity-matcher that
emits a similarity *score* and a human review queue — and the vendor's own caveat is that the score "can't
be interpreted as a probability." We deliberately reject scoring as the primary mechanism, because a
probability is the wrong thing to know. The right thing to know is **where a datum came from and how it got
there**. Linking is done by **explicit, deterministic lenses** — joins on stable keys, normalization lenses
that reconcile naming (`15/9-F-14` ⇄ `NO 15/9-F-14`), depth/time-interval matchers — and the integrity of a
link comes not from a confidence number but from the fact that every derived link records (C1/C5) the exact
lens version and source fields that produced it. A wrong link is therefore not a low-confidence guess to be
re-reviewed; it is a *visible* consequence of a *specific* rule, fixed at the rule and re-derived
deterministically across everything downstream (C1's early cutoff means the fix touches only the blast
radius). Where a judgement genuinely needs a human or an LLM — a fuzzy name no rule resolves — that decision
is itself a governed effect (§9.8) recorded as an edit with provenance: auditable, replayable, attributable,
never an opaque score. The result is integration that is *debuggable* — you can always answer "why is this
linked to that," which is what an operator auditing a drilling decision needs.

### 8.2 Deletion, GC, and retention
An append-only immutable log is in tension with "delete this" and with bounded storage. Three mechanisms,
matched to three needs:

- **Logical delete** (normal). A delete is a `−1` Z-set edit (§9.7) / tombstone (§7): the value leaves the
  live view; history retains it. Cheap and ordinary.
- **Physical erasure** (right-to-be-forgotten, secrets). Genuinely remove bytes. Strategy: keep erasable
  payloads in an **external mutable store keyed by hash/ref**; the immutable log holds only the ref. Erasure
  deletes the external value; the log's ref dangles harmlessly — the *fact that a value existed* remains, the
  value is gone. Mutation is quarantined; the log stays immutable. (The Datomic-GDPR pattern.)
- **Retention = snapshot + truncate.** The log would grow forever; periodic **checkpoints** — and every branch
  root is already a snapshot (§7) — let us **truncate the log prefix** older than the latest snapshot all
  consumers have passed, since replay never needs further back. Snapshots are structurally shared (§7) so they
  stay cheap, and unlike Delta (whose time-travel breaks after VACUUM) **as-of queries survive truncation down
  to snapshot granularity**.

**GC of the content-addressed store** is mark-and-sweep from live branch roots + retained snapshots: an object
no branch or snapshot references is collectable; structural sharing means dropping a branch frees only its
*unique* objects. Old **derivation lineage** can be coarse-grained (keep edit lineage, summarize derivation
detail) to bound growth further.

> **Honest limits.** Erasing a value that fed a *derived* result (an aggregate, an embedding, a model) gives
> erasure a **blast radius** — downstream may need recompute or erasure too. GC sweeps are expensive at scale
> (cf. lakeFS's minutes-long jobs) and run off-peak / incrementally. The "all consumers passed this snapshot"
> condition needs coordination once distributed (deferred with sync, §10).

---

## 9. The dataflow model: bidirectional lenses (C6)

The running application — the data graph (C4) — is a **DAG of lenses**. This is the heart of the system's
*behavior*.

**Map of §9.** The first eleven subsections build the model bottom-up: the **lens** and **node** primitives
(§9.1–9.2), the absence of source/sink (§9.3), **stream types** and composition (§9.4), how the **backward**
direction is authored (§9.5), the single **scheduler** that runs push/pull/optimism/async (§9.6), and the
**edit / provenance** substrate (§9.7). On top sit the harder mechanics — **slow / effectful** lenses (§9.8),
**boundary adapters** (§9.9), lenses **as code** (§9.10), and **parametrization** (§9.11). The rest applies
the model: two end-to-end **worked examples** (§9.12 wind-farm, §9.13 metering), then specialized lens
families — **time-series** (§9.14), **JSON** (§9.15), **time as a demand axis** (§9.16), and **change views**
(§9.17) — closing with **charts** (§9.18) as a capstone on the parameter-space problem. Lens primitives first,
then the catalogue they generate.

### 9.1 Lens
A **lens** is a bidirectional, possibly stateful transform with a **forward** direction (`derive`:
propagate edits downstream) and a **backward** direction (propagate edits upstream). Both carry **deltas,
not whole values** — these are *delta lenses* / *edit lenses* from the literature. Backward also receives
the current upstream state, because most transforms aren't invertible and need context to reconstruct an
upstream edit. When a backward edit *can't* propagate further, the lens **stores it and replays it
downstream** — which is as good as possible and fits the event log naturally.

### 9.2 Node
A **node** is an instantiated lens (a resident, C4): it holds materialized state, provenance (for echo
suppression, §9.7), and its downstream subscribers (for demand, §9.6). *Where* its state lives and *how
durably* is its persistence policy (§7) — configuration, not code.

### 9.3 No source/sink — only boundaries
A **source** is a node with no upstream; a **sink** has no downstream. A **boundary adapter** is a lens
whose outer face speaks an external protocol — a database change feed, an HTTP API, a network peer, a
rendering host. Structurally each is just a lens — so **boundary adapter** and **boundary lens** name the
same thing — and only its outer face is special. This is why the UI
(§11), the network (§10), and the AI (§12) are not separate subsystems — they are boundary lenses on the
same graph (C6).

### 9.4 Stream types and composition
A wire carries a **stream type**: three runtime-checkable schemas — `state` (the materialized value),
`fwdEdit`, `bwdEdit`.

```ts
interface StreamType { state: Schema; fwdEdit: Schema; bwdEdit: Schema }
interface Lens<In extends StreamType, Out extends StreamType> {
  inType: In; outType: Out;
  forward (edit: Edit<In["fwdEdit"]>,  stateIn: State<In["state"]>): Edit<Out["fwdEdit"]>;
  backward(edit: Edit<Out["bwdEdit"]>, stateIn: State<In["state"]>): Edit<In["bwdEdit"]>;
}
```

**Composition `A ∘ B` is legal iff `A.outType` structurally equals `B.inType`** across all three schemas —
the precise bidirectional meaning of "the formats match." Schemas (not TS types) are the **runtime source
of truth**, validated at every port, because lenses are often machine-authored and TS types erase at
runtime. A schema does quadruple duty: wiring-time compatibility, runtime port guard, the authoring spec
handed to an LLM, and the **content-addressing soundness boundary** (C3) where a pure lens meets the world.

**There is no universal node/edge format — deliberately.** **State and edits are typed *per stream* (per
lens)** — a table delta, a JSON patch (§9.15), a cell-edit are all different formats, validated at their ports.
Only two things are uniform: the **dependency topology** (a DAG of nodes-depend-on-nodes — the one sense in
which this is a "graph": a *computation* graph, not a data model), and the **delta envelope** every edit shares
— provenance + origin (§9.7), a lattice timestamp (§9.6), a signed Z-set multiplicity (§9.7). So the event log
(§8) is one ordered log of heterogeneous, per-lens-typed entries unified by that envelope. Even
**relationships** are not a built-in edge primitive: a "connection" is itself typed, derived data from a
linking lens (§8.1) — many link types, never one edge table. The uniformity is in the *algebra of change and
the topology of computation*, never the data model — so "data graph" throughout means the running *dependency*
DAG (§5), not a knowledge graph.

### 9.5 The backward direction is authored, not inferred
A lens's `backward` is **ordinary code the author (or an LLM, §9.10) writes** — never a mapping the system
must *discover*. The academic "view-update problem" is hard only if you demand an automatic, total inverse
of a non-injective transform; we demand neither. Backward is whatever the author declares, and every case a
real graph throws up falls into one of four kinds, each with a clear discipline:

1. **Invertible — write the inverse.** When the forward transform has a sensible inverse, write it. A
   `calibrate` lens (`out = in + offset`) has backward `in = out − offset`; a `unitConvert` backward is the
   reverse conversion; a `rename`/`reshape` backward re-addresses the edit. These satisfy the round-trip
   laws, and the auto-generated round-trip property tests (§9.10) check them.

2. **Not invertible, but the edit is *new state* — route it.** Most "edits" on a derived view are not
   attempts to un-compute a derivation; they are new facts that belong somewhere upstream. An operator who
   acknowledges an alarm is not inverting a health computation (you cannot un-derive a temperature) — they
   are recording an annotation. The lens's backward is then **explicitly programmed to route that edit to the
   right resident store** (an annotation layer, §7), which the forward path joins back in. "Backward" here
   means *place this edit where it belongs*, not *invert f*.

3. **Multi-port — carry the policy as a parameter.** Fan-in/fan-out is the *only* place real ambiguity
   lives, and we resolve it by **configuration, not inference**. A `join`'s backward edit is routed to one or
   both upstreams by a **routing policy** the join carries as a parameter ("writes to fields from A go to A");
   a `fanout`/`merge`'s competing backward edits are ordered/combined by a **merge policy** parameter. The
   author picks the policy; the lens is then total and well-tested. This is precisely the parametrization of
   §9.11 — a general multi-port lens specialized by its backward policy. All ambiguity is quarantined into a
   few named, audited lenses (`join`, `merge`, `fanout`), keeping the authored common case **1-in/1-out**.

4. **No sensible backward — declare it absent.** A projection, a ranking, a holistic aggregate (§3): a
   top-k view cannot be *edited* into a different ranking. Such a lens declares `backward: none`, and the
   stream type (§9.4) makes a write to it a **wire-time error**, not a runtime surprise. Partial backward is
   equally fine — a lens may accept some backward edit kinds and reject others. (When a meaningful backward
   edit reaches a lens that cannot propagate it further, the lens **stores and replays it downstream** per
   §9.1 — the event log absorbs it.)

The discipline, then: **the type declares which directions exist; the author writes backward code where it
makes sense; parameters resolve multi-port ambiguity; round-trip property tests verify whatever laws the lens
claims.** Nothing is auto-discovered, so nothing is mysterious — and because backward is just more pure
`(edit, state) → edit` code, it is as cheap to author and as safe to regenerate as forward (§9.10).

### 9.6 Evaluation: the scheduler
All of §9's pieces — push, pull, optimistic edits, echoes, async results, recursion — run under **one
scheduler with one rule**: *advance the frontier, then recompute the observed blast-radius in dependency
order with early cutoff.* Everything else is a consequence.

**Push marks, pull computes.** Dirtiness propagates eagerly; values are computed lazily. A source change
pushes an *invalidation* wave along subscription edges (cheap — proportional to the dependency-graph blast
radius, not to recompute cost); an **observer** (a mounted view §11, an external puller, or a standing
observer attached to an eager "build" node §1) then *pulls*, walking down with the change/verify stamps of §3
and stopping at early cutoff. This is why unobserved subgraphs cost nothing *and* live views stay current:
the push is the notification, the pull is the work, and the work happens only where something is watching.

**Glitch-freedom by dependency order.** Edits are batched into a **revision** (a logical tick). A node never
observes half-updated inputs because recomputation proceeds in **topological order over the dependency DAG** —
every node's inputs reach a consistent revision before it computes. Dynamic dependencies (a node that reads
different inputs after an edit) are handled as in Adapton: dependency edges are re-recorded each computation
and the order is recomputed from the live graph.

**Partial-order logical time** (resolving the §19 open question). Each edit carries a timestamp in a
**lattice**, typically `(revision, iteration)` — `revision` the input epoch, `iteration` the fixpoint
coordinate (below). A node computes at timestamp *t* once the **frontier** has passed *t* on all inputs (no
edit ≤ *t* can still arrive). This is what makes **retroactive edits** (twin history-rewrites, §8) and
**out-of-order external feeds** (§9.9) correct rather than corrupting: a late edit at an old timestamp
re-opens exactly the affected `(data, time)` cells and re-propagates, bounded by early cutoff — it does not
force a replay from now. Total order would be simpler but cannot express "the past was corrected," which the
twin requires; we pay the partial-order machinery (progress-tracking / frontiers) to get it.

**Cycles are explicit fixpoints, never accidents.** Forward propagation runs over an **acyclic** dependency
DAG. Backward edits (§9.5) do not form scheduler cycles: a backward edit is injected as a *new edit at its
upstream source*, re-entering the front of the pipeline as ordinary forward input — re-entrant, not circular.
The only loops are **explicit recursive/fixpoint lenses** (transitive closure), where the `iteration`
coordinate carries each round and the frontier advances past the loop only once it reaches a least fixed
point. Acyclic forward DAG + re-entrant backward edits + bounded explicit recursion — there is no path to an
accidental cycle.

**Ordering, optimism, echo, async — one mechanism.** Within a revision: (1) incoming edits apply to sources,
their nondeterminism recorded as governed effects (§6) into the log (§8), whose order *is* the canonical
order; (2) the frontier advances; (3) the observed blast-radius recomputes in dependency order with early
cutoff. Layered on this:
- **Optimistic local edits** (§9.7) apply immediately at the resident node, provenance `local`,
  *speculatively ahead of the frontier* — instant UX. When the authoritative edit arrives the optimistic edit
  is **rebased**; if they agree (the common case) early cutoff makes reconciliation a no-op.
- **Echoes** are dropped by provenance / idempotency key *before* re-entering propagation (§9.7) — a write
  returning through a feed is not a new edit.
- **Async / effectful lenses** (§9.8) never block the scheduler — a result is just an edit arriving at a
  later timestamp, handled by the same frontier machinery.

**Determinism is a corollary, not extra work.** Because the log fixes edit order and timestamps (§8) and
governed effects record nondeterminism (§6), **replaying the log reproduces the exact schedule and state** —
the scheduler is a pure function of `(log, code)`. That is what makes §8's time-travel, crash-recovery, and
branch-replay sound, and what lets the AI observe its edit's effect *reproducibly* (§12.3).

> **Honest limits.** Glitch-free topological scheduling needs the live dependency order (cheap in-process;
> the dynamic-dependency graph handles changing edges). Frontier / progress-tracking is real engineering — it
> is differential dataflow's hardest part — and **cross-process / distributed frontier coordination** (§10) is
> harder still and deferred; the model above is specified for the single-process engine first.

### 9.7 Edits, provenance, echo, reconciliation
Every edit carries provenance *and signed multiplicities*:
`{ ops, origin: "local"|"upstream"|"echo", provenance: tag }`, where each op is a row carrying an integer
weight over an abelian group — a delete is `−1`, an insert `+1` — the **Z-set** model shared by every
production incremental engine (differential dataflow, DBSP, Materialize). This is load-bearing, not
cosmetic: aggregation, `distinct`, `except`, and anti/outer-joins are *non-monotone* (an insert to a base
fact can *retract* a derived fact), so without signed weights those operators are wrong under deletion. The
delta substrate carries weights from the bottom up. The
central hazard is **echo / double-apply**: a local edit written back to an external DB returns through that
DB's change feed as if new. Every node recognizes the echo of its own write (by provenance/idempotency key)
and drops it — and the same mechanism suppresses *network* echoes (§10). When optimistic and authoritative
state diverge, pending local edits are **rebased**. Whether edits are commutative (CRDT-style) or need
ordered rebase (OT-style) is the one cross-cutting open decision (it governs both UI reconciliation and
network sync — §18).

> **Validated (C3-proto):** a UI edit flowed backward through a lens, mutated the source, re-derived
> forward, produced **one** minimal output change in 115 µs, echo suppressed.

### 9.8 Slow, effectful, non-reproducible lenses
Some derivations are **expensive and not exactly reproducible** — the canonical case is calling an LLM to
derive, say, a graph from text. These are a distinct flavor, handled by composing C3 and C7:

- **Effect-captured, not recomputed** — the call is a governed effect (C3); its result is recorded into
  the log and becomes the node's state. Replay reuses the artifact; it never re-calls the model.
- **Async** — emits a `pending/stale` delta, then a `resolved` delta. Downstream treats this as ordinary
  deltas (a spinner, stale-while-revalidate).
- **Refreshed explicitly, not auto-invalidated** — an input change marks the node *stale*; *when* to
  recompute is a policy (pull-on-demand, manual, debounce, TTL, stale-while-revalidate). These are
  **pull-points** in an otherwise push graph — your instinct that it is "just when you decide to pull."
- **Memoized by input hash, durable by default** — identical inputs reuse the recorded result; the result
  takes a durable persistence policy (§7) and is shared across branches with the same input hash.

An LLM is, in this model, just another external system at the boundary whose responses we capture into a
replayable edit stream — the same machinery as a database (§9.9).

### 9.9 Boundary adapters: manufacturing a reliable edit stream
A boundary adapter's real job is to **manufacture a reliable edit stream** from an external system that may
not provide one — the deepest engineering in the project. Structurally it is a **lens that is also a resident
node** (§5): its outer face speaks an external protocol; its inner face emits the graph's **timestamped Z-set
deltas** (§9.6, §9.7); and it holds, as resident state, a **durable cursor** and a **provenance/idempotency**
index. Forward = external change → graph edit; backward = graph edit → external write (write-through). Every
adapter, regardless of class, must satisfy one **contract**:

1. **Exactly-once-in-effect.** Every external change becomes exactly one graph edit — no drops, no duplicates
   — *across restarts*. (Delivery may be at-least-once; idempotency by `key + source-position` makes the
   *effect* exactly-once.)
2. **Monotone frontier.** Each emitted edit carries a lattice timestamp (§9.6) derived from the source
   position, and the adapter advances a **frontier** so the scheduler knows when `(key, t)` is settled. This
   is the bridge from a messy external feed to the glitch-free, partial-order engine.
3. **Idempotent, echo-suppressed write-back.** Its own writes returning through the feed are recognized by
   idempotency key and dropped as echoes (§9.7).

**Three classes, by source capability.**
- **Native CDC** — Postgres logical replication / WAL, MySQL binlog, Mongo change streams, Debezium. The
  source already provides an *ordered* log with positions (LSN). Map LSN → timestamp; the **LSN is the
  cursor**; each change → a Z-set delta (`+1`/`−1`; update = retract+assert) tagged with the source txn. Order
  is guaranteed within the stream, so the **frontier is just the last committed LSN** — no diffing, lowest
  latency. *Best class.* Hazards: replication-slot retention / back-pressure (a lagging consumer makes the
  source accumulate WAL — a real ops failure mode) and DDL / schema-change events.
- **Polling + diff** — REST APIs, JDBC tables without CDC, files. Periodically snapshot (or query
  "changed-since" if the source has an `updated_at`), **diff against last-known state by stable key + content
  hash**, emit deltas for the difference. Cursor = the watermark, or a stored Merkle/hash summary of the last
  snapshot when there is none. Timestamp = poll epoch; latency = poll interval; only *net* change between
  polls is visible. Content-hashing (C2) lets unchanged partitions skip the diff entirely. Hazards: **delete
  detection** (needs the full key set each poll, or source tombstones); watermark/clock skew at the interval
  boundary (handled by overlap + idempotency).
- **Write-through / outbox** — when the graph is the *sole writer*, or via the transactional-outbox pattern.
  A backward edit writes the external store **and** records the edit in our log atomically; with no
  independent writers there is no feed to reconcile and the forward stream is trivially known. Simplest
  forward story, but **invalid the moment another writer exists** — then fall back to CDC or polling.

**The two universal hard problems.**
- **Snapshot ↔ stream handover.** Bootstrapping must load the full initial state *and* switch to the live
  stream with no dropped or double-counted edits across the seam. Solution (DBLog / Debezium-style
  watermarking): note the stream start position, take a consistent snapshot at a known LSN, emit snapshot rows
  as inserts, then replay buffered stream events *after* that LSN — deduped by `key + position`. Z-sets make
  the seam self-correcting: a snapshot insert and a stream insert for the same key collapse by multiplicity,
  so idempotent keying absorbs overlap. For large tables, interleave snapshot **chunks** with the stream
  between low/high watermarks. The frontier does not advance past the snapshot LSN until handover completes.
- **Durable cursor + restart.** The cursor (LSN / watermark / Merkle summary) is persisted **transactionally
  with the emitted edits** — it lives in a resident layer (§7) and is itself event-sourced (§8). After a crash
  the adapter resumes at exactly the last acknowledged position and re-emits; idempotency drops anything
  already applied. Exactly-once-in-effect (contract 1) is precisely cursor durability + idempotency.

**The replay boundary.** External effects live *only* in adapters, which is exactly where replay stops (§8).
The captured forward edit stream is recorded as a governed effect (§6, §9.8), so **replay re-emits from the
log and never re-hits the external system**; backward write-through is a real external effect, run in live
mode only and skipped on replay. Recovery therefore never double-writes.

> **Honest limits.** CDC slot retention / back-pressure and DDL handling are operational, not theoretical,
> and must be monitored — this is also where "a stalled extractor" alerting lives. Polling cannot see
> intra-interval states and leans on the source for delete signals. And a backward edit that must *atomically*
> hit two external systems is distributed-transaction territory — out of scope; we offer per-store
> write-through with compensation, not cross-store atomicity.

### 9.10 Lenses as code
Lenses run in the governed runtime (C3). For safe on-demand (LLM) authoring: keep `forward`/`backward`
**pure** in the common case (`(edit, state) → edit` — trivially sandboxable and content-addressable);
take **effects only via injected capabilities**; author **schema-first** (give the model the In/Out stream
types + lens laws + examples; it fills in two functions; port validation and auto-generated round-trip
property tests catch mistakes).

This dissolves a problem that bites content-addressed systems like Unison, where a change re-hashes every
transitive dependent and forces a slow manual cascade of updates. Here propagation is cheap in *both*
directions: an LLM writes a forward or backward transform from the schema in seconds, and a cheaply-generated
transform is *trustworthy* because the schema is the runtime source of truth (§9.4) and every port is guarded
by validation plus auto-generated round-trip property tests. So updating dependents is **regenerate-and-
validate**, not hand-patch-and-pray — the cost of writing code fell, the cost of *trusting* generated code is
paid by the substrate, and together they retire the cascade. The hard part that genuinely remains is not
authoring the new code but **migrating live resident state** across the change (§5) — a real subsystem,
flagged honestly in §19, distinct from and harder than regenerating the pure code that surrounds it.

### 9.11 Parametrizable lenses and specialization
A lens is not just a pair of functions; it is a **parametrizable definition**. A general lens takes
configuration — a key extractor, a matching predicate, a backward policy (§9.5), a persistence policy (§7) —
and a **specific lens is derived by fixing some of those parameters**, exactly as currying a function yields
a more specific function. `join` specialized with a depth-interval predicate is a `depth-join`; a generic
`normalize` specialized with a well-naming table is the WITSML name reconciler (§8.1). Two consequences fall
out of content-addressing (C2): a specialization that coincides with an existing one *is the same object*,
shared automatically; and because the general lens and all its specializations are hashed definitions, the
whole family is branchable, diffable, and reusable. This is the intended authoring economy — a small,
audited library of **general** lenses (the capability-bearing boundary adapters and the named multi-port
lenses of §9.5, where trust and effects live) gives rise to a large catalogue of **specific, pure** lenses an
LLM assembles per use case (where the application logic lives). Generality is curated; specificity is cheap.

### 9.12 Worked example: wind-farm condition monitoring
A concrete, end-to-end use case on **open industrial data** — 10-minute SCADA from public wind-farm datasets
(e.g. the Kelmarsh and Penmanshiel farms, and ENGIE's La Haute Borne). It exercises every layer at once and,
in particular, shows the four backward kinds of §9.5 arising naturally in **one** screen.

**Sources (boundary lenses, §9.9).**
- *SCADA history* — per turbine, ~10-min averages of power, wind speed, rotor RPM, gearbox- and
  generator-bearing temperatures, nacelle position. High-volume numeric: stored in a **columnar engine
  behind a boundary lens**, which manufactures **block-grain** deltas (a new 10-min block per turbine), not
  per-sample events (§9.7).
- *Asset model* — turbine → component (gearbox, generator, blade-pitch) hierarchy + site metadata; a small
  resident graph.
- *Work orders* — maintenance events as an edit stream.

**Forward derivations (lenses).**
- `calibrate` — applies a per-sensor offset to raw temps (**backward kind 1**: invertible).
- `powerCurveDeviation` — bins (wind speed → expected power), emits `actual − expected`. Linear (§3), cheap.
- `tempSlope` — rolling slope of bearing temperature; a **linear** aggregate, O(groups), updated per delta.
- `health` — per component, folds deviations + slopes into `ok | watch | alarm`, **joined** with operator
  annotations (below).
- `fleetTopK` — the *k* unhealthiest turbines on the site. A **holistic** aggregate (top-k): O(rows) state,
  so built with **staged reduction** (§3) and declaring **no backward** (**kind 4**) — you cannot edit a
  ranking.
- `<Table>` — the fleet dashboard (§11.7), windowed and virtualized.

**Backward in action (the point).**
- *Acknowledge / annotate an alarm.* An engineer marks an `alarm` "inspected — sensor drift, not a fault."
  **Kind 2**: `health` does not invert its computation; its backward **routes the annotation to a `user`-layer
  store** (§7) that the forward path joins back in. The dashboard updates optimistically (§9.6) in
  microseconds; the annotation is an edit with provenance (who, when), fully auditable (§8).
- *Correct a calibration.* "Gearbox-temp sensor on T07 reads +3 °C high since 12 Mar." A backward edit through
  `calibrate` (**kind 1**) writes the offset to a calibration overlay (§7); early cutoff (§3) re-derives only
  T07's affected nodes, and because history is event-sourced (§8) the correction applies *as-of* the stated
  date without rewriting the immutable base — the §7 overlay holds the correction; the columnar base stays
  append-only.
- *Close a work order from the dashboard.* The dashboard joins `health ⋈ workOrders`; "close" must land on
  the **work-order** source, not SCADA. The join's **routing policy** (**kind 3**) sends it there — a
  parameter on that one join, no inference.
- *The `fleetTopK` ranking* rejects writes at wire time (**kind 4**).

**The non-functional layers, for free.**
- *Authorization* (§13.1): a site engineer's capability admits only their site's turbines; resolution (§7)
  trims the fleet view to those rows before any lens runs. An OEM's capability might admit gearbox telemetry
  across sites but not power-production figures — column-level, by the same mechanism.
- *Branching* (§7): "what if we re-tune the power curve?" is a 0-copy branch; the re-tuned `powerCurve` and
  its downstream health run beside production and **diff as data** (§11.5) — no copy of the SCADA history.
- *Lineage* (§8): "why is T07 in alarm?" traces to the exact bearing-temp blocks, the calibration offset and
  its correcting edit, the `tempSlope`/`health` lens versions, and the threshold — the auditable answer an
  operator needs, with no probability anywhere (§8.1).

### 9.13 A second case, in brief: metering → settlement
A different shape — integration-heavy and aggregation-heavy — on open smart-meter data (e.g. the UK Low
Carbon London and Irish CER trials). Three sources are **linked deterministically** (§8.1): half-hourly meter
reads, a customer/tariff master, and a network-loss-factor table — joined on meter-id and effective-date,
with a normalization lens reconciling id formats, *no scores*. Forward derivations are mostly **linear
aggregates** (sum consumption per customer per period — cheap, O(customers)); the **demand charge** is `max`
over the period — a **holistic** aggregate (§3), built with staged reduction and flagged as the costly node.
The interesting backward edit is a **billing dispute**: a correction ("this read was estimated; here is the
actual") is **kind 2** — it does not invert the bill, it routes an adjustment to a correction layer (§7),
re-derives only that customer's settlement (early cutoff, §3), and leaves an auditable lineage from corrected
read → adjusted bill (§8). Authorization is naturally **per-customer row-level** (§13.1): a customer sees only
their own reads; a settlement agent sees aggregates but, by column policy, not personal identifiers. This is
the *same machinery* as the wind farm pointed at a billing problem — which is the unification claim (§16) made
concrete rather than asserted.

### 9.14 Time series and bulk numeric data
The V8 object graph is the wrong representation for billions of numeric samples (LAS curves, SCADA, production
histories). The design delegates storage and owns derivation:

- **A distinct resident node type backed by columnar, compressed, chunked storage** — in practice an
  **external columnar engine behind a boundary lens** (§9.9): DuckDB/Parquet, ClickHouse, or a purpose-built
  TSDB. The runtime delegates compression and compaction (incumbents reach ~3 bytes/datapoint); it keeps
  derivation, provenance, and incrementality.
- **The log grain is the chunk, not the sample.** An adapter (§9.9) emits "a new chunk of N samples for series
  X at frontier *t*," never per-sample events — keeping the event log (§8) cheap and provenance at
  series/chunk grain (per-sample lineage is cost without benefit).
- **Derived metrics are lenses over chunks**, maintained incrementally: downsamples, rolling windows,
  continuous aggregates. Linear aggregates (§3) update per chunk; holistic ones use staged reduction. This is
  exactly a TimescaleDB *continuous aggregate* — expressed as our lens on delegated columnar storage.
- **Downsampling pyramids.** Pre-materialize progressively coarser resolutions (Cognite's interval tree, PI's
  compression) as derived nodes (§3), incrementally maintained as chunks land, so a coarse range query reads
  O(result), not O(raw).
- **Corrections live in overlay layers (§7), not the columnar base.** A recalibration or late sample is a
  tombstone+replacement in an upper layer; resolution (§7) merges base + correction so readers see the
  corrected series without rewriting the append-only/compressed base; periodic **compaction** folds
  corrections down on the columnar engine's own schedule. This resolves the "backward write into
  append-optimized columnar is correction-hostile" tension directly.
- **Depth- and time-indexed series are one node type keyed differently** (LAS by measured depth, SCADA by
  time).

> **Honest limits.** Out-of-order / late samples re-open closed windows — handled by the §9.6 frontier
> (watermarks / allowed-lateness), with genuinely-late data below the bound dropped. Vector / ANN indexes are
> a *separate* delegated index type with poor incremental deletion (they assume mostly-append). Cross-machine
> float determinism in aggregates is deferred (§6.1).

### 9.15 Structured (JSON) lenses
JSON is the lingua franca of config, documents, API payloads, and — in this system — the UI tree itself
(§11.11), so **bidirectional lenses over JSON documents are first-class**. A JSON lens is just a lens (§9.4)
whose In/Out *state* schemas describe JSON shapes, with a library of common transforms: `project` (pick
fields), `reshape`/`rename`, `filter`, `merge`, `wrap`/`unwrap`, `default`. Forward maps a source JSON delta
to a view JSON delta; backward follows §9.5's four kinds — most JSON transforms are cleanly invertible (kind 1:
`rename` re-addresses the edit, `project` routes a field write back), and the non-invertible ones declare a
policy or `none`. Deltas are **structural** (JSON-Patch / RFC-6902-style ops over paths), not whole-document
replacements, so editing one field of a large document is one op, and §3 early-cutoff and §11.12
virtualization apply to documents exactly as to tables. Because the schema is the runtime source of truth
(§9.4), a JSON lens is validated, content-addressed (§4), and authorable in all three modes (§11.10) like any
lens. This is what lets the UI document (§11.11), a config file, an external API payload (§9.9), and a stored
record be edited *through views* with the edit propagating back to source — JSON in, JSON out, both directions.

### 9.16 Time as a demand axis: as-of queries, ranges, and smooth scrubbing
Time is not special — it is **another demand axis**, exactly like the spatial viewport of §11.12. A view
observes a slice of *time* (a point or a window), that slice is resident state (§7), and moving it re-derives
only the delta. So "query data for a time range" and "teleport smoothly over time" (Foxglove-style playback)
are the *same* mechanism — virtualization, in time.

**Two temporal queries, both first-class lens inputs.**
- **As-of (point-in-time)** — the state of the world at instant *T*: the materialized view of all edits with
  timestamp ≤ *T* (§9.6), the whole twin "rewound." A lens takes *T* as input and produces the data valid then.
- **Over-a-range (windowed)** — data within `[T1, T2]`: events in an hour, a time-series segment, a timeline.
  The window is the demand; the lens is parameterized by the range (§9.11) and may aggregate over it (§9.14).

**Smooth scrubbing is incremental, and Z-sets make it bidirectional.** Moving the cursor from `T1` to `T2`
does *not* replay from genesis: forward, the engine applies the edits in `(T1, T2]` — its ordinary delta
propagation (§9.6), cost ∝ the changes in the interval. **Backward is cheap because Z-set deltas are
invertible** (a `+1` reverses to `−1`, the abelian-group structure of §9.7), so rewinding applies inverse
deltas rather than rebuilding. An arbitrary jump lands on the **nearest snapshot ≤ T** (§8, §7) and replays
forward — bounded by snapshot density, a tunable. Seeking is fast because the log and storage are
**time-indexed** (sequencer timestamps §15.1, MCAP-style chunk indexes, §9.14 pyramids), so locating the edits
in `(T1, T2]` or the nearest snapshot ≤ T is a lookup, not a scan.

**Playback and a shared playhead.** Playback is just advancing the time-demand on a clock — real-time or
accelerated — emitting a frame per tick; the UI renders only the per-frame delta (§11.12, B1-minimal
mutations), so it is smooth. A single **playhead** is one piece of session state (§7) that *every* time-aware
view observes, so scrubbing it moves the 3-D scene, the plots, the tables, and the timeline (§11.15) to the
same as-of instant at once — Foxglove's synchronized cross-panel playback, here because they share one demand.

> **Honest limits.** Rewinding shows *recorded past state*; it does not un-send external effects — replay
> stops at boundary adapters (§8, §9.9), so the past you scrub is the twin's, not the outside world's. A jump
> with no near snapshot pays replay (snapshot density trades storage for seek latency). Smooth playback assumes
> a small per-frame delta (§11.12); a frame where everything changes repaints, and a holistic aggregate over a
> sliding window is the §3 cost reappearing.

### 9.17 Change views: diff, recently-changed, and blame
Showing what changed is not a feature to add — the system *already knows* exactly what changed, because every
edit is a delta with provenance (§9.7), a timestamp (§9.6), and a branch (§7). "Review the changes," "mark
what's recently changed," and "who changed this" are therefore **lenses over information the substrate already
carries**, rendered as overlays on any view.

- **Diff = the delta between two states, as data.** The change-set of a branch is the edits on its branch
  layer over its base (§7); a diff between two times is the edits in `(T1, T2]` (§9.16); a diff between two
  branches is their reconciled difference (§11.5, B2). All are the *same* "delta between two states" lens, and
  because deltas are Z-sets (§9.7) a diff is literally signed — additions `+1`, removals `−1`, updates as
  before/after. This is the review model of §11.10 made into a first-class view.
- **Review as an overlay on any visualization.** A diff is not a separate screen; it is a **decoration lens**
  composed onto a view (§11.6, §11.14): a changed table cell glows, a moved diagram node highlights, a
  re-parented tree node flags, an added document span is marked. Accept / reject is a backward edit — promote
  or revert (§7, §13.1) — so review happens *in place*, in whatever visualization fits the data.
- **Recently-changed = "since T" overlay.** Each user's "last seen" is session state (§7); "what's new" is the
  range query `timestamp > lastSeen` (§9.16) rendered as a highlight or badge. The same mechanism gives
  "changed in the last hour," "changed since this branch forked," or a live pulse as deltas arrive (§11.12).
- **Blame is intrinsic.** Because provenance is carried, not reconstructed (§8), every datum shows *who or
  what* (a person, or which AI step, §12.4), *when*, and — through lineage — *why* and *from which sources*.
  This is `git blame` generalized to every field, surfaced on every view.

The point is uniformity: diff, review markers, recently-changed, and blame are one capability — **render the
change information the delta + provenance + time model already holds** — not four bolted-on features, and they
work on *every* visualization (§11.15) because they are lenses, not table-specific chrome.

### 9.18 Worked example: charts and the parameter-space problem
A chart is the sharpest test of authoring, because its parameter space is enormous — chart type, x/y mappings,
scales, binning, grouping, stacking, aggregation, color, legend, thresholds, annotations, time window, tooltip
format, and more. The model tames it the same way it tames everything: the space *is* a config, narrowing *is*
specialization, and the three authoring modes (§11.10) each address a different slice.

**The whole space is one config (JSON, §9.15).** A chart is a component-lens (§11.15) whose configuration is a
large JSON document. So *every serializable parameter* — types, field-paths, scales, ranges, colors, bins — is
a field you edit **through the UI**: a config form is a JSON-lens-backed component (§11.14), edits round-trip
backward (§9.5) onto the config, and the chart re-renders incrementally. "Edit all serializable parameters in
the UI" is therefore free — it is just editing a JSON document through a view.

**Pre-defined narrowing lenses (parametrize, §9.11).** Nobody configures the full space from scratch. A
**narrowing lens** fixes most of it and exposes a small, meaningful subset: `timeSeriesLine` fixes type=line,
x=time and exposes `{series, yAxis, window}`; `assetHealthChart` narrows further to a domain. These are
specializations (§9.11) — general `chart` → narrowed → domain-narrowed — each layer collapsing dimensionality,
shipped as a preset library and shared by hash (C2).

**AI-generated narrowing lenses (vibe-code, §11.10 / §12.4).** Given the data and an intent ("show anomalies
in gearbox temp this week"), the AI authors a narrowing lens *schema-first* (§9.10): the chart config schema
(§9.4) is the spec it fills, so it picks type, series, scale, binning, and annotations. Its output is just a
lens config (data, §9.15) — reviewable, branchable, editable — so the AI proposes a chart and you nudge two
fields in the UI and accept (§12.4, propose-don't-impose).

**Serializable vs. code is the parametrize/code line (§11.10).** The split is exactly the authoring split:
**serializable** parameters (enums, paths, numbers, colors) are data, editable in the generic config form;
**non-serializable** ones (a custom scale transform, a bespoke tooltip formatter) are *functions* — authored
in the code mode (§11.10) as the lens's own code, not exposed as form fields. The same artifact carries both;
the UI edits the data part and drops to code for the rest, with no seam.

So a chart is configured along a continuum — pick a narrowing preset, let the AI narrow further, hand-edit the
serializable fields, drop to code for a custom transform — all producing one content-addressed chart lens (§4),
validated by its schema (§9.4), live (§1). The huge parameter space is not a UI problem; it is a
*narrowing-by-specialization* problem the lens model already solves.

---

## 10. Distribution and sync

Because *everything* is deltas-with-provenance over a DAG (C5, C6), distribution needs no new mechanism. A
**network link is a boundary lens** (§9.3) whose outer face is a remote peer; edits serialize both ways and
the rest of the graph is unaware a wire crosses machines. Sync is **demand-driven** (a peer subscribes only
to the slices it observes — a client to the window it renders), so only the deltas for what's viewed are
shipped — the §9.6 push model over the wire. Bespoke sync engines (Replicache, ElectricSQL, local-first
CRDT stacks) re-implement exactly this; here it is the runtime. **Echo suppression generalizes** to
peer-to-peer (provenance makes replication exactly-once in effect). **Branches are the unit of sync**:
a peer holds a branch; syncing is merging branches — conflict-free per-definition for code (C2),
per-record reconciliation for data (§9.7). **Local-first/offline** is then automatic: a client is a branch
with its own log that reconciles on reconnect. Because a link is just a lens between two branches, there is
**no privileged server**: machines sync **peer-to-peer**, with no cloud or central node in the path — the
topology is whatever the lenses wire (star, mesh, or a single air-gapped pair), so a sensitive twin can run
fully on-prem or air-gapped, syncing only between trusted peers. The conflict model (CRDT vs OT) is the one
real decision (§18), and commutative edits are preferred wherever the data model allows.

---

## 11. The frontend framework (the DOM as a boundary)

The frontend is not a separate system: it is the lens DAG (§9) with a **rendering host as the terminal
boundary lens** (§9.3) and the engine (§3) doing the work. The whole UI — every window, panel, and widget —
is **one derived value**: a JSON-like *UI tree* materialized by lenses from application and session state
(§9), maintained incrementally (§3). The DOM is not the UI; it is a host that *applies the deltas* of that
derived tree. `UI = f(state)`, taken literally and kept incremental (the OS-like consequences are §11.11).

**Map of this section.** It builds in one arc. The DOM is I/O, not a render target (§11.1–11.5). Components are
bidirectional lenses (§11.6) — the table is the worked example (§11.7), edits round-trip backward (§11.8), and
the host is swappable (§11.9). The studio is an IDE that authors lenses (§11.10), running on an OS-like,
derived, server-side UI (§11.11) that is virtualized and realtime (§11.12) and mountable as files (§11.13).
Composite components wired by schemas (§11.14) generate the full visualization catalogue (§11.15); and at the
top, applications themselves are lenses (§11.16) — closing the loop back to the spreadsheet engine of §3.

### 11.1 The DOM is I/O, not a render target
React re-runs a component to build a virtual DOM, then *diffs* it to find what changed — incidental work
that exists only because it discarded the knowledge of what changed. We keep that knowledge: a view is a
**materialized view maintained incrementally** — a *data delta* produces a *DOM delta* directly. No VDOM,
no diff. Work is proportional to what changed, never to view size.

> **Validated (B1):** on a 5000-item dataset with a 50-row window, the median DOM mutations per data edit
> was **0** (most edits don't touch the visible window); a single visible change produced **exactly 1**;
> re-render-and-diff paid ~100 every time.

The boundary is two streams: forward = a **mutation stream** applied by a host; backward = **input events**
turned into intents flowing upstream. The host is dumb and swappable (§11.9), so the framework never talks
to a specific renderer.

### 11.2 The view DAG
A screen is a DAG of derived nodes ending at the host: `sources → join → filter → sortBy → <Table> →
host`. Demand-driven (C1), an unmounted view computes nothing; with early cutoff, a source change that
doesn't affect the visible window produces no render work.

### 11.3 The mutation protocol (forward IR), against logical keys
```ts
type Key = string; // stable logical identity, NOT a DOM position (§11.5)
type Mutation =
  | { op:'create'; key:Key; tag:string; parent:Key; index:number }
  | { op:'remove'; key:Key } | { op:'move'; key:Key; parent:Key; index:number }
  | { op:'setText'; key:Key; text:string }
  | { op:'setAttr'; key:Key; name:string; value:string|null }
  | { op:'setProp'; key:Key; name:string; value:unknown }   // e.g. an input's .value
  | { op:'listen'; key:Key; type:string };                  // declare a subscription
```
A host keeps `Map<Key, Node>` and applies these. That is the whole contract.

### 11.4 The event protocol (backward)
`type UIEvent = { target: Key; type: string; payload: unknown }`. The host translates a native event into
a `UIEvent` keyed by logical identity and pushes it upstream; the owning component (§11.6) turns it into an
intent.

### 11.5 Logical keys
Elements are addressed by stable identity (`row:42`, `cell:42:price`), never by position. This makes
incremental **moves** correct (reorder preserves focus/scroll/input state) and lets an **event log replay
against a differently-rendered branch** (`cell:42:price` still resolves) — the basis for differential UI
testing.

> **Validated (B2):** one event log replayed against two differently-sorted branches yielded divergent,
> diffable mutation streams.

### 11.6 Components are bidirectional lenses
```ts
interface Component<In> {
  render(delta: Delta<In>, ctx: RenderCtx): Mutation[];   // forward: data delta -> subtree mutations
  onEvent?(ev: UIEvent, state: In): Edit<In> | null;      // backward: host event -> domain-local edit (this component's bwdEdit)
}
```
`render` receives a *delta* and emits only the implied mutations (a price change emits one `setText`, not a
re-render). **A component speaks only its own domain.** Its forward input is domain-shaped state and its
backward output is a **domain-local edit in its own vocabulary** (its `bwdEdit`, §9.4) — never an
application- or source-level intent. The host translates native events into the component's event vocabulary
(§11.4); translating *that* onward to the source is the job of the **backward direction of the upstream lens
chain** (§9.5), each lens converting only between its adjacent domains. So components compose because they are
lenses (§9.4) — and they are *reusable* because none of them knows the domain it is ultimately editing.

### 11.7 The table (worked example)
The first-class `<Table>` is prototype C2: an **incremental materialized index** keyed by `(sortKey, id)`
plus a window of the top *K* rows. A data `update` re-evaluates **one** record (filter membership, sort
position, cell text), updates the index in O(log N), recomputes the window, emits the tiny diff — cost
independent of total rows. **Virtualization is free**: the window is a *demand*; scrolling changes which
rows are demanded, so the engine computes only newly-visible rows.

> **Validated (C2):** single-record edits cost **7.9 µs end-to-end**, 27× faster than re-render and
> independent of N; redefining a column/sort triggers a windowed rebuild (~1 ms); editing an unused
> definition costs nothing (early cutoff).

**The table speaks only "table"** — the load-bearing discipline (§11.6) that makes `<Table>` reusable across
every domain. Its stream type (§9.4) is purely tabular: **state** = rows × columns + sort + window;
**fwdEdit** = a table delta (a cell changed, a row entered/left the window, a re-sort); **bwdEdit** = a
**table event** — `cell-edit{row, col, value}`, `sort{col, dir}`, `select{rows}`, `scroll{window}`. The host
hands it generic native events keyed by logical identity (§11.4); the table translates those into *table
events* and nothing more — it knows tables and the host, never the application or the source. A
`cell-edit{row:42, col:price}` then **propagates backward through the lens chain** (§9.5), each lens
translating only its adjacent domains: `sortBy` passes it through (sort doesn't change cell identity),
`filter` passes it through, a computed-column lens inverts its formula into edits on the columns it read, a
`join` routes it to the owning upstream (§9.5 kind 3), and the source finally applies it. That is why the *same* `<Table>` drives
a drilling fleet (§9.12) and a billing run (§9.13) unchanged: the lenses adapt the domain to the table, never
the table to the domain.

### 11.8 UI edits: local handling + backward round-trip + configurable persistence
A keystroke does **two things at once, deliberately separate**: (1) it is **handled locally, immediately**
— the input's resident state updates so the box shows what was typed with zero latency, regardless of how
far the edit propagates; and (2) it **round-trips backward** as an intent toward wherever the truth lives.
*Where* the truth lives is the persistence policy (§7), not the component's concern: the same typing can be
wired local-only (scratch), event-sourced (durable, replayable), or write-through (to an external source) —
by changing one policy on a node, not the component's code. Reconciliation (§9.7) handles divergence (the
server normalizes `9.9`→`9.90`); only differing cells re-emit. This is prototype C3 in its home.

### 11.9 Swappable host → rendering targets, testing, AI observation
The mutation IR (§11.3) is host-agnostic, so the same view DAG drives different **targets** by swapping the
boundary lens: **DOM**, **terminal/TUI**, **native** (UIKit/Android/retained scene graph), **canvas/WebGL**,
**server-rendered HTML** (apply mutations server-side, stream to a thin client — literally §10), **PDF/print**,
or **headless** (tests, AI). One view can drive several targets at once (browser + recorder + AI projection),
since each is just another subscriber. A **recording host** captures both streams, enabling differential UI
testing (§11.5) and AI observation (§12).

### 11.10 The studio — an extensible IDE over the graph
The flagship human surface is an **IDE-like studio**: build views, drive the system by chat, and operate the
twin in one workspace. It is not a special application — it is a **workspace of view DAGs and boundary lenses
(§9, §11) over the same graph**, with the AI (§12) embedded as a participant and plugins as content-addressed
definitions (§4) in the governed sandbox (§13).

**Scope — substrate, not product.** We deliberately do *not* fully specify the studio: it is an *illustrative
consumer*, and many UIs will sit on the same substrate. The spec's job is only the small **contract a UI like
this builds on**, which is already present in the layers above:

- **The UI has two jobs: author lenses and consume them.** A "view" is a lens chain ending in a UI component
  (§11.6); *building* a view is authoring or specializing lenses (§9.10, §9.11) through the namespace API
  (§12.1); *using* a view is subscribing a host (§11.9). Author-and-consume is the whole of it — everything a
  UI does reduces to making lenses and rendering them.
- **Every task is its own branch.** As in Claude Code for desktop, each unit of work — building a view, an AI
  edit session, an integration attempt — opens a **0-copy branch** (§7): it diverges only by what it writes,
  runs beside production, and is discarded or merged. Concurrency is free (§7's million-branch result), so a UI
  can keep many speculative tasks live at once.
- **Review is diff + promotion.** Reviewing a task is diffing its branch *as data* (§11.5) and **promoting**
  it through the layer stack (§7) under the authorization gate (§13.1) — one primitive, identical whether a
  human or the AI authored the branch.

Those three are the build target, and they are substrate guarantees, not studio features. The rest of this
section is *illustrative texture* — four properties that show the contract is sufficient by falling out of the
substrate rather than being separately built:

**Every panel is a view; every view is *editable*.** A table, a graph view, a hierarchy/tree, a chart, a
diagram, a document viewer — each is a rendering component (§11.6), i.e. a **bidirectional lens**: forward =
data delta → visual mutation (§11.3), backward = interaction → intent/edit (§11.4). "Editable visualization"
is therefore not a feature built N times; it is §9.5 applied per visualization — the full per-widget
catalogue, with each one's backward kind, is §11.15. You implement edit-support *once* (the backward direction) and it is uniform across every widget. Layout and
docking are themselves state in a layer (§7), so a workspace branches and is shared like any other state.

**Building lenses is the core loop — three authoring modes on one artifact.** A view is a lens chain ending
in a visualization, so *authoring a view is authoring lenses*. The studio offers a continuum, not a fixed
editor:
- **Parametrize** — configure an existing general lens (§9.11): pick a source, set the key / predicate / sort
  / backward policy, drop a viz. No code; direct manipulation; the common case. The studio writes the
  specialized definition to a **branch layer** (§7), live (no build, §1), with blast-radius feedback (§3).
- **Code** — drop to the lens's `forward`/`backward` functions directly (§9.10) where parametrization can't
  reach. The full-power developer path, in the same workspace, on the same branch.
- **Vibe-code** — describe the lens in natural language; the embedded AI (§12) authors it schema-first (§9.10)
  and reads the *same* incremental projections (§12.3), so the user watches the effect and iterates in words.
  Chat is just a panel whose backward direction is "intent → graph edit."

The unifying fact: **all three produce the same content-addressed lens definition (§4), validated identically**
— schema/port guards plus auto-generated round-trip property tests (§9.4, §9.10) — so the authoring mode is a
*UX choice, never a correctness boundary*. You can parametrize a lens, drop to code for one function, then ask
the AI to refine it, and the artifact and its safety are unchanged. This is exactly what makes vibe-coding
safe: the validation does not care who or what wrote the code, and a machine-authored lens passes the same
gate as a hand-written one. Human and AI edit the same branch and **diff/merge as data** (§11.5, §7).

This is the curation loop made concrete: **onboarding** (land a source in a layer, reconcile by lens §8.1),
**review** (diff a branch, promote through the §7/§13.1 gate), and **view authoring** are the *same* IDE,
because all three are edits-on-a-branch over one graph.

**Extensibility = more content-addressed definitions, capability-sandboxed.** A plugin is not a privileged
escape hatch; it is more substrate:
- a **new visualization** is a rendering component (§11.6) shipped as a definition (§4), shared by hash;
- a **new transform/analytic** is a lens — pure ones need no capabilities and are trivially safe (§13),
  effectful ones get scoped, audited capabilities;
- a **new tool, panel, or data source** is a boundary lens (§9.9) or view.

Plugins run in the governed sandbox with **only the capabilities granted** (§13): a third-party visualization
that should see only the rows the user may see gets exactly that (§13.1) — user-installed plugins are safe by
the *capability model*, not by trust. Because plugins are content-addressed they version, branch, and share
like everything else; the "marketplace" is just the namespace. The same machinery that sandboxes LLM-authored
lenses sandboxes user plugins — which is why open extensibility is cheap here and dangerous in systems that
secure code by review rather than by capability.

**One view, many observers** (§11.9) — the editor canvas, a recording host, and an AI projection (§12.3)
subscribe at once, so the assistant *sees exactly what the user sees* and can act in context.

**What this still needs (honest).** This is a product-design surface, not only a mechanism: the docking/layout
model, the direct-manipulation authoring UX, and a plugin install / permission-grant flow are real design
work (the visualization library itself is the §11.15 build). The claim is not that the studio is free — its
hard properties (everything editable, extensible, sandboxed, human + AI co-authoring) are *consequences* of
the substrate, so the remaining work is widgets and UX over a model that already supports them, not inventing
the model.

### 11.11 The UI is an operating system: one derived document, server-side, thin client
Three commitments make the whole UI layer fall out of the substrate rather than being a separate app
framework.

**The UI state tree *is* derived data.** There is no privileged "component instance tree" beside the data
graph — the entire UI is a single **derived JSON-like document** (windows, panels, widgets, focus, z-order,
selection, layout), the materialized output of a lens chain over application + session state (§9), maintained
incrementally (§3, §11.1). Editing the UI (drag a window, focus a field, resize a pane) is an edit on that
derived document that round-trips backward (§9.5) to wherever the truth lives (§7) — the UI tree is
bidirectional like any other view. This collapses "UI framework" into "more lenses over more derived state":
there is nothing in the UI that is not also data in the graph.

**The shell is a window manager, and it too is derived.** The top-level environment is OS-like — a **window
manager** compositing many applications, with focus, stacking, tiling/docking, and inter-window data flow.
None of it is special-cased: windows are nodes in the UI document, the layout is state in a layer (§7,
branchable per §11.10), and one application's output can be wired as another's input because they are lenses
on one graph. The studio (§11.10) is *one application* inside this shell; the shell is the same
derived-document model one level up.

**Server-side generation, minimal wire.** By default the UI document and its deltas are computed
**server-side** in the runtime (on V8, §14); the client is **thin** and does exactly two things: apply the
forward **mutation stream** (§11.3, keyed by logical identity §11.5) and send back the backward **event
stream** (§11.4). Only minimal DOM patches go out and minimal events come back — never markup re-renders,
never client-side application logic. This is §11.9's server-rendered target made the *default*, with §10's "a
wire is just a boundary lens" carrying it: the network sits transparently between the derived UI tree and its
host. Consequences fall out: the client is trivially swappable (browser, native, TUI — §11.9) and
near-stateless; the authoritative UI lives where the data and governance do (§13.1); optimistic local
interaction is the §9.7 path applied to the UI document.

> **Honest note.** Server-driven UI trades a round-trip for centralization. The §9.7 optimistic path hides
> latency for local interactions (typing, dragging); genuinely offline-first clients run the UI lens locally
> as a branch (§10) and reconcile — so "server-side by default" is a placement policy (§7), not a hard
> requirement. Per-interaction latency budgets and which widgets must be client-local are real UX design work
> (cf. §11.10's note).

### 11.12 Virtualized by default, realtime without disruption
Two cross-cutting UI properties fall out of the same machinery, and both are essential for a twin over
arbitrarily large, continuously-changing data.

**Virtualization is a general property, not a table feature.** *Demand is state.* Every visual component
observes only a **slice** — a list's viewport, a tree's expanded nodes, a graph's visible region, a
timeline's window, a document's page — and that slice is resident state (§7). Because the engine is
demand-driven (§3), it computes only the observed slice; the backing data may be **arbitrarily large at no
render or compute cost**. Scrolling, expanding, or panning *changes the demand*, and the engine computes
exactly the newly-visible nodes and drops the ones that left — work is proportional to the **viewport**,
never to the dataset. The table (§11.7) is just the first instance; the same "window is a demand" mechanism
virtualizes lists, trees, graph canvases, timelines, and documents uniformly. A billion-row source, a
million-node graph, a multi-year series (§9.14) all render in constant work, because only the demanded slice
is ever materialized — §3's "unobserved subgraphs cost nothing," now at the pixel.

**Realtime without disrupting the user.** New data arrives as deltas pushed to observers (§9.6); incremental
rendering (§11.1) emits only the **minimal mutations** they imply (B1: median **0** per edit, **1** per
visible change), so a live feed never triggers a re-render. Three things together make streaming
*non-disruptive*:
- **Stable logical keys** (§11.5) — elements are addressed by identity, not position, so inserts, deletes,
  and reorders preserve **focus, scroll, selection, and partially-typed input**: the row you are editing stays
  put and stays focused while the list reorders around it.
- **Ephemeral state is never clobbered** — what the user is *currently typing or viewing* lives in the
  ephemeral layer (§7); incoming authoritative deltas **rebase** (§9.7), they do not overwrite the in-progress
  edit. The quantity being typed joins the live view (§7) without the live view stomping it.
- **Out-of-viewport updates are free** — a delta outside the demanded slice produces **zero** mutations (B1),
  so a storm of background changes cannot jank the visible frame.

How live updates *surface* is a configurable policy, not a behavior the framework imposes: stick-to-bottom vs.
pin-scroll-position, stale-while-revalidate (§9.8), batch/debounce/coalesce a high-frequency feed, or a "N new
items — click to load" gate. These are policies on the demand and the lens (§9.8), so one component is a calm
dashboard or a live tail by configuration alone.

> **Honest note.** Constant-work virtualization assumes the *slice* is cheap to locate — true for indexed
> sources (§11.7's `(sortKey, id)` index, the §9.14 pyramids), but not for an ordering that changes globally
> on every edit (a re-sort by a volatile aggregate — the §3 holistic-aggregate cost reappears). Realtime
> smoothness likewise assumes a small per-delta blast radius; a delta that genuinely changes the whole visible
> slice (a global re-sort) must repaint it.

### 11.13 The filesystem as a host: mount the graph, detect changes both ways
Not every UI is a rendered surface; much tooling is **file-oriented** (editors, CLIs, git, scripts, language
servers). The same content-addressed, layered namespace the AI works against (§12.1) is exposed as a **virtual
filesystem the OS can mount** — generalizing the §12.1 projection from an AI feature into a first-class
**bidirectional host**. A definition or data view is a file; a namespace is a directory; opening resolves a
name through the layer stack (§7); saving writes to a layer by policy (§7).

It is bidirectional because it is a **filesystem boundary adapter** (§9.9):
- **forward** — a graph change becomes a file create/modify/delete event, so a watching editor or `git status`
  *sees* the change (change detection outward);
- **backward** — an external file save is detected (native `inotify` / `FSEvents`, or poll+hash, §9.9) and
  becomes an edit into the graph (change detection inward).

The §9.9 contract carries over directly: a durable cursor, and **echo suppression** (§9.7) so our own
write-back does not return through the watcher as a new edit (the classic write→watch→re-apply loop). Because
the projection is just another host over the same UI/definition document (§11.11), file tools, the web studio,
and the AI all act on **one** graph — edit a lens in vim and watch the web view update; the file, the panel,
and the AI projection are three subscribers (§11.9). For non-web or headless environments, this *is* the UI.

### 11.14 Composite components: cells, rows, columns, and the schema
A table is not a monolith; it **composes smaller component-lenses**, and reasoning about how exposes a general
pattern.

**Cells, rows, and columns are themselves component-lenses (§11.6), and they are heterogeneous.** Each
*column* carries its own **cell-lens** — an editable number, a status badge, a sparkline (a §9.14 mini-view),
a dropdown, even a nested `<Table>`. So a table's cells are *not* one representation; column A's cells are a
different lens from column B's. The table delegates: it routes a `(row, col)` value delta to that column's
cell-lens (which emits the cell's sub-mutations) and collects that lens's backward events. The table never
knows what a cell *is* — the "speaks only its domain" discipline (§11.6), now **recursive**: the table speaks
"rows/columns/cells as opaque sub-views," each cell-lens speaks its own value domain, and a cell may itself be
a table (fractal composition).

**Two schemas meet here.** There is the **stream-type schema** (§9.4 — the runtime state/fwdEdit/bwdEdit
contract) and the **table presentation schema**: ordered columns, each `{path, header, type, format,
cell-lens, sortable, editable, width}`. The presentation schema is *derived from* the row data's schema
(auto-columns from fields) and then **overlaid** with user customization — §7 layering applied to the schema
itself (a default-derived value with a user overlay). And it is **editable JSON (§9.15)**: adding, removing,
reordering a column or swapping a cell-lens is a backward edit on the schema document, live (§1). The schema is
therefore both the **wiring** (it binds each column's cell-lens to a field) and a **parameter** (§9.11) —
`<Table>` is one general lens specialized by its schema.

**The deep structure: row-major × column-major, meeting at the cell.** The three decompositions are not one
hierarchy — a cell belongs to both a row and a column, so neither "table ⊃ rows ⊃ cells" nor "table ⊃ columns
⊃ cells" is privileged; which is the composition unit depends on the operation. **Rendering and virtualization
are row-major** (virtualize visible rows, §11.12; the §11.7 index is keyed by `(sortKey, id)`). **Typing and
cell-component binding are column-major** (the schema). **Editing is cell-level** (the unit of a value edit);
selection may be cell, row, column, or range. The table's real job is maintaining this **2-D composition
incrementally** — a row-major data index crossed with a column-major schema, meeting at cells that each defer
to a column's lens. Forward is **fan-out** (a row delta → its visible cells → host mutations); backward is
**fan-in** (cell / header / row events assembled into table events → upstream, §11.7).

**This generalizes.** "A container component fans out to child component-lenses, wired by an editable
schema/layout that is itself data" is not table-specific: a *form* composes field-lenses by a form schema; a
*tree* composes node-lenses; the **window manager (§11.11)** composes application-lenses by a layout. The
table is the worked example of the composite-component pattern, and the window manager is the same pattern at
the top of the UI.

### 11.15 A visualization catalogue — all the same shape
The visual library is large but **uniform in shape**: every visualization is a bidirectional component-lens
(§11.6) with a domain state + event vocabulary, **editable** via §9.5's four backward kinds (`k1` invertible /
`k2` new-state-routed / `k3` multi-port / `k4` none), **virtualized + realtime** (§11.12), **composed** from
sub-lenses by a schema (§11.14), authored in three modes (§11.10), and shippable as a content-addressed plugin.

| Visualization | Domain state | Backward / what "editable" means | Virtualized by | Composed from |
|---|---|---|---|---|
| **Table / list** | rows × cols + sort + window | cell-edit, sort, reorder, select (§11.7) | row window | cell / row / column lenses (§11.14) |
| **Document + annotations** | structured doc (§9.15) + annotation overlay | edit text (k1/k2); add/move annotation (k2 → §7 layer); span→entity extract (§9.8) | visible blocks / pages | block lenses + annotation overlay |
| **Image** | image ref + 2-D markers / boxes | draw / label / move box (k2); pan-zoom (view state) | tiles / zoom level | image host + annotation lenses |
| **Video** | clip ref + playhead + timed events | scrub (state); add timed annotation (k2); play (ephemeral) | time window | video host + timeline overlay; time-indexed source (§9.14) |
| **Timeline / Gantt** | intervals on a time axis + window | move / resize interval → edit start/end (k1); create; select | time window | track lenses + interval lenses (§9.14) |
| **Calendar** | events by date/time + range | move / resize → edit time (k1); create; edit | date range | day / week cell lenses + event lenses |
| **Kanban** | items grouped by a status field into lanes | **drag card → backward-edits the group field (k1)**; reorder (order field); edit card | per-lane window | `groupBy(field)` lens + lane lists + card lenses |
| **Chart** | series + scales + domain viewport | mostly read-only (k4); editable: drag point → edit value (k1), drag threshold → edit param, brush → selection | domain window (downsample pyramids §9.14) | axis / series / mark lenses |
| **Hierarchy / tree** | node tree + expanded set + window | re-parent (move); rename; add / remove; expand (view state) | expanded ∩ visible | recursive node lenses (a node may be any component) |
| **Diagram / graph (P&ID, schematic)** | nodes + edges + layout positions | move node (position); add / reconnect edge (k2/k3 → topology); edit node | visible region | node + edge lenses + layout |
| **Map (geo)** | geo features + viewport | move / draw feature (k2); select | geo tiles / zoom | layer lenses + feature lenses |
| **3-D scene** | scene graph + camera | transform / select entity (k1/k2); camera (view state) | frustum / LOD | entity lenses (retained scene-graph host §11.9) |
| **Form** | record + field schema | per-field edits (k1/k2) | — | field lenses by schema (§11.14) |
| **Gauge / metric tile** | a scalar (often an aggregate §3) | read-only (k4) | — | leaf lens |

Two are worth calling out because they make the model visible. **Kanban** is a `groupBy(statusField)` lens
feeding per-lane lists; dragging a card across lanes is, mechanically, a **backward edit that rewrites the
card's group field** (§9.5 k1) — "editable board" is not a feature, it is what the backward direction *is*.
**Charts** are mostly derived aggregates and so declare `backward: none` (k4), but a raw-series chart is
editable (drag a point → edit the source value, k1), and a million-point series renders smoothly only because
the domain viewport demands the right pyramid level (§9.14) — virtualization-by-resolution, not just by count.

> **Honest note (carried from §11.10).** Uniform *shape* does not make the library free: each widget is a
> real build — hit-testing, layout, accessibility, gesture handling — even though all share the lens/host/
> schema scaffolding. The claim is that editability, virtualization, realtime, and composition come from the
> substrate, so the per-widget work is presentation, not plumbing.

### 11.16 The OS as a UI: applications are lenses
The window manager (§11.11) does not host *programs*; it hosts **applications that are themselves compositions
of lenses**. An application is a named, content-addressed definition (§4) — domain lenses + components + a
schema (§11.14) + a window layout — that the shell instantiates. "Installing an app" adds a definition; apps
branch, version, share, and are authored in all three modes (§11.10) like everything else. Classic desktop
applications map cleanly:

- **Spreadsheet (Excel) is the canonical app, because the engine already *is* a spreadsheet (§3).** A sheet is
  the grid component (§11.7, §11.14) over cells that are nodes holding a literal *or a formula* — a lens
  deriving from other cells. Recalculation is incremental propagation with early cutoff (§3); a formula is a
  lens authored by typing, by code, or by vibe-coding (§11.10); what-if / goal-seek is a backward edit (§9.5);
  a circular reference is an explicit fixpoint (§9.6). Excel is nearly free here — it is the engine's native
  surface, the §3 opening line ("a spreadsheet taken seriously") made into an app.
- **Word / document apps** are a structured-document JSON lens (§9.15) bound to the document+annotations
  component (§11.15); styles and templates are lenses; comments are an annotation layer (§7); track-changes
  *is* the edit log (§8); real-time co-editing *is* branches + sync (§10).
- The same pattern covers a **CAD / diagram** app (geometry + topology lenses, §11.15), a **BI** app (charts
  over aggregates, §3), and **mail / kanban / calendar** apps — each just packages domain-specific lenses and
  components.

**The payoff is interop without files.** Because every app is lenses over *one* graph (§7), a table in the
spreadsheet app and a table embedded in the document are the *same* lens over the *same* data — "paste" is
**wiring, not copying**, and editing either updates both, live (§1), with no file format, import, or export
between applications. This is §16's seam-elimination applied *between apps*: the thing classic operating
systems never achieved (OLE / copy-paste are lossy bridges between siloed programs) is native here, because
there are no silos — only one graph, surfaced through different application-lenses. The OS is then just the
**namespace (§12.1) + window manager (§11.11) + applications-as-lenses + a host (§11.9/§11.13)** — renderable
to the web as a server-side thin client (§11.11), to native, or mounted as files.

> **Honest note.** The *architecture* of these apps is free — data, recalc, undo (the log, §8), collaboration,
> and cross-app interop fall out — but the *long tail* of a mature app (pivot tables, page layout, a thousand
> formulas) is still a real build, the §11.15 note one level up. The claim is that you build application
> *features*, never application *plumbing*, and never the *seams between applications*.

---

## 12. The AI interface

The AI is a first-class participant — both an **author of graph** and a **consumer of graph** — not an
external tool. Three pieces, each built from earlier commitments.

### 12.1 The AI's filesystem
The AI works against the **content-addressed, layered, branchable namespace itself** (C2, C7), exposed two
ways: a **programmatic API** (read/write definitions, query data, fork branches, run lenses) as the
primary interface, and a **virtual-filesystem projection** (synthesized paths) for file-oriented tools and
editors. It is *git's object model + a working tree, at definition granularity, with the data layers
included and 0-copy branching, live (no build)*: a "file" is a definition or a data view; a "directory" is
a namespace; "open" resolves a name through the layer stack (§7); "save" writes to a layer by policy;
"branch" forks at 0-copy. Access is **capability-scoped** (§13) — the AI sees and touches only what it is
granted. This single tree unifies VCS + filesystem + data store.

### 12.2 Chaining AI steps
An AI step *is* a slow/effectful lens (§9.8), so a multi-step workflow is a **pipeline of such lenses** —
and inherits four properties for free:
- **Resumable / incrementally re-run** — each step is memoized by input hash with a recorded output, so
  editing step 3 of 5 re-runs only steps 3–5; steps 1–2 reuse results. **No redundant model calls.**
- **Reproducible** — the chain replays from the log deterministically (C3), never re-hitting the model.
- **Branchable** — run three variant step-3 prompts as three branches sharing steps 1–2 (0-copy), and
  **diff their outputs as data** (§11.5/B2).
- **Lineage-tracked** — every step traces to its inputs + prompt version + recorded response (§8).

Each step runs in a capability-scoped sandbox (§13). AI workflows are thus *just lens DAGs with expensive
nodes*, getting memoization, branching, and lineage from the substrate rather than a bespoke orchestrator.

### 12.3 Projections the AI reads
Instead of pixels or raw dumps, we produce purpose-built **derived views — themselves lenses** — that
present the running system to the AI as structured, queryable, *incremental* data: "the current view as a
semantic tree," "the mutation stream since my last edit," "the data feeding this component," "the diff
between branch A and B." Because they are incremental, the AI observes *deltas* — what changed because of
its edit — in microseconds and deterministically. This closes the loop the whole substrate exists for: the
AI edits a lens (code graph) → observes the structured projection of the effect (data graph) → in
microseconds, reproducibly. The edit→observe latency the runtime fights for is, ultimately, this loop's.

### 12.4 The proactive AI: fill, clean, watch, schedule
The AI is not only summoned (§12.1–12.3); it also **works on its own**, and the same substrate that makes its
reactive edits safe makes its proactive ones safe.

**Fill and clean.** Missing data is imputed or extracted (a value pulled from a document, a field inferred
from neighbours); dirty data is normalized, deduplicated, and reconciled. Each is a **governed-effect lens**
(§9.8): the model call is recorded with provenance (§9.7), so a fill or a clean is an *edit you can trace,
review, branch, and undo* — never an opaque overwrite.

**Watch.** Because the graph is incremental and push-based (§9.6), the AI can **stand as an observer** and
react to *deltas*, not re-scans: an issue-detector lens (unresolved entities, low-confidence links,
contradictions, anomalies, schema drift, stale derivations) re-evaluates only what changed (§3), so
continuous monitoring of an arbitrarily large twin is cheap. Issues surface as **derived facts** — a queue
the user browses (§11.12) and the AI works down — not an alerting system bolted on the side.

**Schedule and trigger.** A workflow is an AI step-chain (§12.2) run on a **clock or a condition** — a
pull-point (§9.8) with a schedule/trigger policy: "every night, re-extract changed documents," "when a new
source lands, propose links," "when quality on this asset drops, open an issue." Each run is memoized,
reproducible, and branched (§12.2), so scheduled work never re-does settled work and never acts irreversibly.

**The discipline: propose, don't impose.** Everything proactive lands as **edits-with-provenance on a branch**
(§9.7, §7), reviewed and promoted by the §13.1 gate — so an always-on AI can aggressively raise data quality
without any unreviewable or unrecoverable action. This is the §1 flywheel made autonomous — an always-on AI
raising the data quality every use-case depends on, with a human in the loop exactly where judgement is needed.

---

## 13. Safe execution and sandboxing

Running machine-authored JavaScript safely is a **layered defense**, chosen per trust level — and most of
it falls out of C3:

1. **Capability-based (default).** A curated context with **no ambient authority**: no `import`, no
   un-governed globals (C3 deletes/freezes nondeterminism), effects only via an injected `ctx`. Since the
   common lens is pure `(edit,state)→edit`, **most lenses need no capabilities and are trivially safe.**
   Only boundary adapters get capabilities, and those are pre-built and audited, not LLM-authored.
2. **Context isolation.** Each sandbox/branch is a V8 Context with its own frozen globals; one cannot reach
   another's state.
3. **Isolate isolation + limits.** For untrusted/expensive/parallel code, a separate Isolate (separate
   heap) with a heap cap and a termination watchdog (CPU/timeout), plus a token/time budget for LLM steps.
4. **Process isolation.** For genuinely hostile code where Spectre-class cross-Context side channels matter,
   a separate OS process — the strong boundary, at higher cost.

Two principles tie it together: **the capability model is the real boundary** — safety comes from *what you
hand the code*, not from sanitizing the code, which is the only approach that survives machine-authored JS;
and **determinism is itself a security property** — governed effects mean even malicious code cannot reach
the clock, entropy, or network except through audited capabilities, blunting timing attacks and exfiltration.

### 13.1 Authorization is per-edit and row-level (a separate axis from sandboxing)
Sandboxing answers *what may this code do*; authorization answers *who may read or write this datum* — a
distinct axis we treat as first-class, not an afterthought. Incumbent industrial platforms authorize
*coarsely*: a grant is a `(resource-type, scope, action)` triple assigned to a group, so "read time series in
this data set" is about the finest grain on offer, and the same fact's access is restated in a vocabulary
separate from its lineage. Because in our model **every datum is an edit with provenance (C5) resolved
through a stack of layers (C7)**, we can do strictly better — and reuse machinery we already have rather than
bolt on a parallel access-control system:

- **Every edit already records who authored it.** Provenance (§9.7) carries origin; authorization is the
  same fact read as *who is permitted to have caused this*. Writes are checked against the author's
  capability at the granularity of the individual edit.
- **Reads are capability-filtered during layer resolution.** The top-down walk (§7) that merges layers into
  one view simply skips rows (and cells) the reader's capability does not admit — so a derivation (§9) sees a
  view already trimmed to what the principal may see, *without the lens knowing authorization exists*. Row-
  and cell-level security is a property of resolution, not a filter wrapped around it.
- **Policies are themselves derivations.** "An operator may read events for wells in their field" is a lens
  over the same graph; access decisions are therefore incremental, branchable, and **auditable through the
  very same lineage as the data** (§8). The audit trail is not a separate log — it is the edit log read
  through the authorization lens.
- **Write-protection by layer.** A principal may write to the *branch* or *user* layer but not to *shared*
  (§7), so an AI agent or third party can propose and round-trip edits (§11.8) with no path to corrupt the
  system of record; **promotion** (§7) is the governed, lineage-tracked gate between layers — the equivalent
  of an incumbent's write-protected dataset, but expressed in the same primitives as everything else.

This is finer than the group/scope/action model *and* intrinsic. The enterprise table stakes still sit on
top — IdP/OIDC federation, org-level tenant isolation, and the audit-retention and certification posture
(SOC 2 / ISO 27001) buyers gate procurement on — but those are deployment concerns layered over this core,
not substitutes for it. Naming them here is deliberate: a data platform that treats them as out-of-scope is
not credible to an industrial buyer.

---

## 14. The execution substrate: V8

We run on **V8** via Rust `rusty_v8`, and reimplement the runtime *around* it. Node is rejected precisely
because its value — the module system, event loop, and standard library — is the file- and batch-oriented
machinery we are replacing (Node's module identity is a path; we need a content hash, C2). V8's JIT, GC,
and object model are a near-perfect fit and insane to reimplement, and it offers two primitives that map
directly onto our model: **Isolate = one shared heap** (so pure nodes are shared across branches by
reference, C4) and **Context = one branch's/sandbox's globals** (C4, §13). Per-Context globals being ours
to define is what makes effect governance (C3) clean.

> **Validated (V1–V3):** cross-Context heap sharing works (V2); rebind→observe is **42 µs** (V3). But a
> Context costs ~157 KB (V1) — so a Context is *not* the branch unit (branches are root pointers, §7),
> and execution is decoupled from branch identity (a pool of Isolates evaluates against any branch's
> immutable snapshot — branch count doesn't bound parallelism). One sharp constraint: rebinding a function
> the JIT had *inlined* into a hot caller de-optimizes it **permanently (~4.5×)** — V8 won't re-inline what
> it learned is unstable.

That last point forces a **two-tier model**: **live/edit mode** accepts the de-opt tax (still hundreds of
millions of ops/sec — irrelevant beside a 42 µs loop); **ship/freeze mode** stops swapping, lets V8 inline
freely, and regains full speed. This is the one place the substrate dictates an architectural choice.

---

## 15. Versioning, storage, and language

These are consequences of C2/C3, recorded for completeness:

- **Version control is our own**, at definition granularity (C2): content-addressed definitions, names as a
  separate namespace, conflict-free merge. Git is supported as an export/interop projection, not the live
  store.
- **Storage** is a content-addressed object store with lazy hydration into the heap; the OS filesystem
  appears only as backing bytes (like git packfiles) and as the virtual-FS projection (§12.1). No live
  filesystem dependency.
- **Language is JavaScript syntax with restricted, governed semantics** (C3) — not a new language, to keep
  LLM fluency and tooling. **TypeScript is optional authoring sugar**: type-stripping is a cheap local
  per-definition transform; full type-checking, if wanted, reuses the TS language server as an incremental
  node (C1), not a reimplementation. **Schemas are the runtime source of truth** (§9.4), validated at every
  port, because erased TS types can't be trusted and lenses may be machine-authored.

### 15.1 Storage architecture: a sequenced log over content-addressed blobs
The physical storage story is **three stores with three jobs, plus a sequencer that orders writes.**

**The sequenced log — the spine (fintech-style).** The source of truth (§8) is a durable, append-only,
*ordered* log. Order is assigned by a **sequencer**: a single writer *per branch / partition* that stamps each
edit with a monotonic sequence number and a lattice timestamp (§9.6), persists it durably (append-before-ack,
WAL-style), and hands it to consumers — the **LMAX-Disruptor / exchange-matching-engine** pattern (single
writer, ring buffer, batched durable append, downstream consumers materializing views) that sustains millions
of ops/sec on one machine. Single-writer-per-branch is *why* writes serialize (the §19 ceiling) and *how*
replay is deterministic (one canonical order, §8/§9.6). Each entry is the **delta envelope** (provenance,
timestamp, Z-set multiplicity — §9.4) plus a payload that is small-inline or a **hash reference** to a blob.

**Total order within a writer, partial order across writers.** A *global* sequencer would be simplest but
breaks P2P/no-server (§10). So order is **per-branch**: each branch (or peer) runs its own sequencer giving a
local total order (deterministic, fast); *across* branches, partial-order timestamps + frontiers (§9.6) and
merge (§18) reconcile. This is the fintech sequencer *and* the distributed model, each where it fits — and it
needs no central server (a peer sequences its own writes locally, §10).

**The content-addressed object store (CAS), blob-backed.** Immutable objects — definitions (C2), values,
persistent-map nodes (§7), snapshots — keyed by hash, deduplicated, and (being immutable) cacheable anywhere
with no invalidation. **Large objects go to a blob store** (S3 / GCS / Azure, or local): images, video,
documents, columnar chunks (§9.14), ML artifacts. **Small objects are packed** (LSM / packfile-style, à la
git) to avoid per-object overhead. Lazy hydration pulls objects into the heap on demand (§15); cold objects
evict.

**Disposable materialized state.** The current resolved views, the engine's arrangements / indexes (§9.6), and
hot data live in a **rebuildable cache** (in-heap + a fast local KV like RocksDB) — *not* a source of truth.
Crash recovery replays the log from the nearest snapshot (§8) to rebuild it; it can be dropped freely.

**Snapshots, blobs, and erasure tie it together.** A snapshot is a content-addressed branch root (structural
sharing, §7), so snapshots are cheap and let the log **truncate** its prefix (§8.2). The blob store also holds
the one mutable exception — the erasable-payload store for right-to-be-forgotten (§8.2), keyed by ref so the
immutable log keeps only a (dangling-after-erasure) pointer. Time series bypass the object graph entirely into
the delegated columnar engine (§9.14).

> **Honest note.** This composes proven pieces — a sequencer (LMAX / Kafka / Raft-log), content-addressed
> storage (git / Nix / IPFS), blob stores, an LSM cache — rather than inventing a storage engine; the work is
> integration and write-path durability/throughput tuning. Scaling a *single* branch's write throughput beyond
> one sequencer is the §19 horizontal-write-scale question, and remains the unproven part.

---

## 16. Why it hangs together

Every capability traces to the commitments, and the cross-cutting features are *combinations*, not
add-ons:

- **0-copy branching** = C4 (pure/resident) + C7 (layered persistent state).
- **Crash recovery & time-travel** = C5 (deltas) + C3 (deterministic replay) + §7 snapshots.
- **Lineage** = C2 (content hashes) + C1 (dependency edges) + C5 (the log).
- **Network sync** = C5 + C6 (a peer is a boundary lens).
- **The AI filesystem** = C2 + C7 (the namespace + layer stack, projected).
- **AI step chains** = §9.8 (slow lenses) + C7 (memoized, durable) + §13 (sandboxed).
- **Sandboxing** = C3 (governed effects) + capabilities + V8 isolation.
- **The incremental UI** = C1 + C6 (the DOM is a boundary lens).

Incumbents keep these concerns in *separate, un-unified products*: a lakehouse splits versioning, lineage +
governance, incremental compute, and AI across four systems; a cloud digital twin splits the twin's state
from its history from its ingestion functions; an industrial historian splits the time-series archive from
the asset model from the audit log. Each seam is a place where identity, provenance, and access must be
re-stated and re-reconciled in a different vocabulary — and the result *feels* disjointed to use: lineage
that stops at a system boundary, a branch that covers only one layer, permissions defined four times. Our
wager is the opposite: because lineage, incrementality, versioning, and authorization all fall out of *one*
graph of deltas, the system is coherent *by construction*, not integrated after the fact. Unification is not
merely cheaper to build — the coherence it produces is itself the feature, and it is the thing four bolted-
together products structurally cannot deliver.

Seven commitments, one graph.

---

## 17. What's validated (with numbers)

Each row is a running prototype, not a projection.

| Prototype | Question | Result |
|---|---|---|
| **A1** | Edit cost = blast radius? | ~3% of graph per typical edit; **0** downstream on no-op-output |
| **B1** | Render cost = output delta? | median **0** mutations/edit; **1** per visible change; vs ~100 re-render |
| **B2** | Cross-branch event replay? | one log → divergent, diffable mutation streams |
| **V1/V2/V3** | V8 as substrate? | cross-context sharing works; **42 µs** rebind→observe; Context **157 KB** (not the branch unit); **~4.5×** permanent hot-path de-opt → two-tier |
| **C1/C2/C3** | Unified live loop? | code+data one path; **8 µs** blast-bounded edit; **115 µs** write-back |
| **overlay** | Millions of branches? | **1M @ ~2.8 KB, 0.7 µs fork** (30,000× vs copy) |
| **layers** | Layered state + lifecycle? | 200k branches over shared base @ ~1.7 KB, **0.25 µs**; top-down resolution + tombstones + wholesale ephemeral-drop |
| **effects** | Sound hashing + replay? | identical across branches; log reproduces exactly; escape-proof; statically screenable |

### 17.1 Implementation-readiness map
Status per subsystem: **✓ prototyped** (running code, above) · **◑ specified** (designed in enough detail to
build) · **○ design-work** (named, not yet designed). Honest summary: *the substrate is specified end-to-end; what remains design-work is the product
surfaces (studio, human query language, operator shell) and the named scale/ops residuals inside ◑ rows.*

| Subsystem | Status | Ref / note |
|---|---|---|
| Incremental engine (early cutoff, stamps) | ✓ | §3 (A1) |
| Scheduler — push/pull, glitch-freedom, partial-order time | ◑ | §9.6 — specified, not yet built |
| Content-addressed definitions | ◑ | §4 |
| Layered state + 0-copy branching | ✓ | §7 (overlay, layers) |
| Governed effects | ✓ core / ◑ completeness | §6 (effects proto); completeness §6.1 (conformance suite ongoing) |
| Event log + lineage | ◑ | §8 |
| Storage architecture (sequencer / CAS / blobs) | ◑ | §15.1; single-writer ceiling + write-path tuning ○ |
| Lenses fwd/bwd (incl. §9.5 method) | ◑ | §9; *multi-port policy library* ○ |
| Z-set / signed-multiplicity deltas | ◑ | §9.7 |
| Aggregation (linear vs holistic, staged reduction) | ◑ | §3 |
| Boundary-adapter internals (CDC / poll / outbox) | ◑ | §9.9 — specified; ops monitoring + cross-store atomicity ○ |
| Time-series / columnar subsystem | ◑ | §9.14 — delegated columnar + chunk-grain log + pyramids |
| Incremental UI (IVM, mutation IR, `<Table>`) | ✓ | §11 (B1/B2/C1–C3) |
| Swappable hosts | ◑ | §11.9 (DOM proto; other targets ○) |
| Studio IDE + visualization library | ○ | §11.10 — UX + widgets |
| AI interface (filesystem, chaining, projections) | ◑ | §12; "projection beats screenshots" unmeasured (§19) |
| Authorization (per-edit, row-level) | ◑ | §13.1 |
| Sandboxing (capability → process) | ◑ | §13; V8 isolation ✓ (§14) |
| V8 substrate (sharing, rebind, de-opt) | ✓ | §14 (V1–V3) |
| State migration on hot-swap | ◑ | §5.1 — explicit/tested/reversible; large-state cost ○ |
| Distribution / sync | ◑ | §10; conflict model = decision-rule only (§19) |
| Deletion / GC / retention | ◑ | §8.2 — logical/physical/retention; GC-at-scale ○ |
| Human query language (compiles to lenses) | ○ | not yet designed |
| Operator twin shell | ○ | not yet designed |

The critical path to a *runnable* system is the **◑ scheduler** (§9.6) plus one **boundary adapter**
(§9.9) — which is exactly the §18 first milestone. The **○ product surfaces** are what turn the runtime into
a usable twin, and are the largest remaining tranche of work.

---

## 18. First milestone

The smallest loop that exercises every real pressure point at once:

> one real DB adapter (Postgres CDC or polling) → a couple of pure lenses → a `join` → a `<Table>`, with a
> local cell edit **handled locally and** round-tripping through the join into the DB, its CDC echo
> suppressed, and the cell's persistence policy chosen by config.

This forces: stream types (§9.4), multi-port backward routing (§9.5), change-manufacturing over an external
DB (§9.9), echo suppression (§9.7), configurable state placement (§7, §11.8), and the incremental table +
host (§11). Everything else is more lens, component, adapter, and target types.

---

## 19. Open problems and honest risks

Several entries below are now *specified* (the design is written; only residual risk remains); the rest are
genuinely open. Both are listed — honesty about the open ones is what makes the specified ones credible.

- **Aggregations defeat incrementality** (§3) — a real algorithmic limit; mitigable, not removable.
- **Governance completeness** (C3) — *specified (§6.1)*: three dispositions (delete / replace+freeze /
  canonicalize) over a finite enumerable set, enforced by static screening + a per-V8-version conformance
  suite. Residual risk: cross-machine float determinism (deferred) and version-fragility of the set.
- **The conflict model** (CRDT vs OT) — feeds both UI reconciliation (§9.7) and network sync (§10). The
  decision *rule* is known, not open: with a trusted central authority at merge time, server-authoritative /
  OT-lite is leaner; only offline-first / peer-to-peer genuinely needs CRDTs, which pay a memory + tombstone-
  GC tax. Decide per deployment rather than globally.
- **Deletion vs. the append-only log** — *specified (§8.2)*: logical delete (tombstone), physical erasure
  (external-mutable payloads keyed by ref), retention by snapshot+truncate, GC by mark-sweep over branch
  roots. Residual risk: erasure has a downstream blast radius, and GC sweeps are expensive at scale.
- **Time-order model** — *specified (§9.6): partial-order logical time* (lattice timestamps + frontiers),
  because retroactive edits (twin history-rewrites) require expressing "the past was corrected," which total
  order cannot. The residual risk is cost, not choice: progress-tracking is real engineering, and
  **cross-process / distributed frontier coordination** (§10) is harder still and deferred to after the
  single-process engine.
- **Horizontal write scale** — no comparable immutable/branchable store (Datomic's single transactor, Dolt's
  "must fit on one machine", TerminusDB's memory-bound graph) scales *writes* horizontally; branching is
  0-copy but writes still serialize. If we claim horizontal write scale it is the novel, unproven part — not
  a solved one — and should be flagged as such, not assumed.
- **GC churn and JIT de-opt at sustained scale** — unmeasured under combined heavy-edit + live-data load.
- **The "structured projection beats screenshots for an AI" thesis** (§12.3) — argued, not yet measured
  against a real agent task; the experiment most able to change the cost/benefit calculus.
- **External-DB change capture** — *specified (§9.9)*: the adapter contract (exactly-once-in-effect, monotone
  frontier, idempotent write-back), three source classes, snapshot↔stream handover, durable cursor. Residual
  risk is operational (CDC slot back-pressure, DDL, monitoring) and read-only-first still simplifies write-back.
- **Time-series at scale** — *specified (§9.14)*: delegated columnar storage behind a boundary lens,
  chunk-grain log, incremental continuous aggregates, downsampling pyramids, corrections in overlay layers.
  Residual risk: late-data window re-opening and the deferred cross-machine float-determinism question.
- **Live state migration** — *specified (§5.1)*: state-compatible swap / migration function / rebuild-from-
  inputs, branch-isolated and reversible. Residual risk: large-state migrations are non-incremental (cost ∝
  state), and general semantic-preserving migration is unsolved (as in any schema migration).
- **Operational maturity** — debugging, stack traces, and observability across hot-swaps, branches, and
  peers must be built; event sourcing gives reproducibility but not yet the tooling.

---

## 20. Prior art — and what is actually novel

Every individual pillar has mature prior art, and a sophisticated reader will recognize each on sight. The
honest novelty is the **unification** (§16), not any one primitive — so we lead with that and never with a
mechanism a reviewer can name. Stated plainly:

| Commitment | Closest prior art | Novel here? |
|---|---|---|
| C1 — incremental graph | Salsa, Adapton, differential dataflow, DBSP, Jane Street `incremental` | No — a chosen point in a well-mapped space |
| C2 — content-addressed code | Unison, Nix, Git | No — Unison's "big idea," applied to a live data/twin runtime |
| C5 — delta log as truth | Datomic, XTDB, Delta Lake | Partly — *durable, unbounded, queryable-as-of*, vs. incumbents' bounded retention |
| C7 — 0-copy layered branches | Dolt, lakeFS, Irmin, Foundry Global Branching | Partly — only because we branch *and merge at every layer uniformly*, which incumbents do not |
| C6 — bidirectional lenses | delta & edit lenses (Diskin; Foster/Pierce Boomerang) | **Yes** — no production data platform pushes derived edits back through the view-update problem; recombined PL theory, net-new at this scale |
| the whole | *(none combine all of the above on one substrate)* | **Yes** — seam-elimination is the thesis |

Underlying references: Salsa / Adapton / differential dataflow / DBSP / Jane Street `incremental` (incremental
computation); Unison, Nix and Git (content-addressed code/objects); Datomic / XTDB / Delta Lake (immutable
log + time-travel); Dolt / lakeFS / Irmin / TerminusDB (git-for-data branching); delta & edit lenses — Diskin
et al., Foster/Hofmann/Pierce/Wagner (bidirectional transforms); Debezium and native DB change feeds (CDC);
Replicache / ElectricSQL / Yjs / CRDTs / OT (local-first sync); Redux-optimistic, CodeMirror transaction
origins (optimistic updates with provenance). The bidirectional row is the genuinely differentiated capability, and §9.5
shows why it is also *tractable*: backward is authored per lens — invertible, routed to new state,
parametrized at multi-port nodes, or declared absent — never an automatic total inverse, so it carries far
less risk than the academic "view-update problem" implies.

---

## 21. Glossary

- **Incremental computation** — recompute only what changed; stop when an output is unchanged.
- **Demand-driven / early cutoff** — compute only what's asked for; halt propagation on unchanged output.
- **Blast radius** — the nodes whose outputs genuinely change after an edit.
- **Content-addressed** — identified by a hash of content, so identical things are automatically shared.
- **Definition / namespace** — the unit of code (hashed) / the name→hash table that points at definitions.
- **Pure / resident node** — stateless-and-shareable vs stateful-and-per-branch.
- **Governed effect** — a nondeterministic primitive swapped for a recorded/replayed, frozen version.
- **Injected capability** — the only door to outside effects, handed to code rather than imported.
- **Branch** — a speculative system version: a namespace overlay + state root pointers; 0-copy.
- **Layered store / layer / tombstone** — a top-down-resolved stack of persistent maps / one such map /
  a marker that hides a key in lower layers.
- **Persistence policy** — the configurable choice of which layer / how durably a node's state lives.
- **MVCC** — versioned, structurally-shared state; cheap snapshots and forks.
- **Lens / delta (edit) / provenance** — a bidirectional delta transform / the unit of change on a wire /
  the tag that identifies an edit's origin.
- **Stream type** — the three schemas (state, fwdEdit, bwdEdit) a wire carries; governs composition.
- **Z-set / signed multiplicity** — a multiset whose rows carry signed integer weights (insert `+1`, delete
  `−1`); the delta algebra that keeps aggregation, `distinct`, and deletion correct (§9.7).
- **Frontier / partial-order time** — lattice timestamps (e.g. `(revision, iteration)`) with a frontier
  marking when a timestamp is settled; lets retroactive and out-of-order edits stay correct (§9.6).
- **Boundary adapter** — a lens whose outer face speaks an external protocol (DB feed, peer, render host).
- **CDC** — manufacturing an ordered edit stream from an external database.
- **Echo suppression** — dropping a feed's or peer's re-emission of your own write.
- **Event sourcing / lineage** — the edit log as source of truth / the traceable history and derivation of
  any value.
- **IVM** — incremental view maintenance: a data delta becomes a view delta with no diffing.
- **Mutation stream / rendering host / logical key** — the host-agnostic forward UI IR / its swappable
  interpreter (DOM, TUI, native…) / an element's position-independent identity.
- **Two-tier (live/ship)** — editable-but-de-optimized vs frozen-and-fully-optimized execution.
