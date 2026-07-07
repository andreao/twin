# An Incremental Runtime

A working implementation of the digital-twin substrate in [`design_doc.md`](./design_doc.md):
*one local-first incremental computation graph that anyone can pour data into, that
derives and connects, that any number of use-cases sit on.*

## The twin: a UI + an autonomous local-model agent

On top of the substrate runs **`twin serve`** — an agent workspace where the AI is in
charge and you steer (à la Claude Code): you open nearly blind and grow the twin
together. Everything is state *in the twin* — the chat log, the agent's thoughts, the
growing user profile, and the mounted data sources are all graph edits in the event
log (§8). The browser is a thin host that applies the forward mutation IR (§11.3) and
sends back UI events (§11.4); the whole UI is a derived document computed server-side
in V8 (§11.11).

- **Agent (§12), thin and substrate-native.** No bespoke orchestrator: the agent
  perceives the graph as a structured projection (§12.3) and acts by emitting
  tool-calls that become graph edits (§12.1). A model call is one governed slow
  effect (§9.8) — an HTTP call to a **local** model (Ollama), driven by our own
  prompt-based tool protocol so it stays model-agnostic. It works between
  conversations too: quiet periods become background turns (profiling sources,
  annotating fields, authoring lenses, keeping its agenda), backing off when idle.
  Its tools cover conversation, rendering live views inline, mounting and
  profiling sources, authoring lenses, reading documents, semantic search over
  what it read, and running its own agenda.
- **Schema inference on mount.** Every source is statistically profiled as it
  mounts (types, keys, cross-source references, enums, patterns); the agent layers
  on the semantics the statistics can't know, so joins land on the right keys.
- **Industrial formats mount as rows.** Beyond CSV/JSON, the boundary readers
  flatten WITSML and EDM XML, LAS well logs, xlsx workbooks and fixed-width
  listings — hand-rolled, still zero deps. Documents climb a ladder: PDF text
  layer, else page images read by the local vision model (OCR); the text is
  embedded locally for semantic search. Linking stays deterministic (§8.1): lens
  code gets `normalizeWell` and `inInterval`, so well names and depth intervals
  reconcile across systems — no similarity scores.
- **Data residence spectrum (§7 / §9.9 / §15.1).** "All data is *in the twin*" is a
  *logical* guarantee: a source is **mounted** (federated — a boundary adapter over the
  external file, no copy) by default, and *could* be extracted / selectively synced /
  materialized into a twin layer on demand. Residence is a per-layer policy; change-
  management and branching are uniform across the whole spectrum.
- **Local model.** Defaults to `gemma4:12b` via Ollama; override with `TWIN_MODEL`
  (e.g. `TWIN_MODEL=gemma3:27b`). Zero new dependencies — the Ollama HTTP client and
  the WebSocket are hand-rolled (still just `v8` / `sha2` / `serde_json`).

```bash
ollama serve                             # the local model host, on :11434
cargo run --bin serve                    # open http://127.0.0.1:8080
```

## Architecture: Rust around raw V8 (not Node)

The runtime is **Rust driving raw V8 via `rusty_v8`**, exactly as §14 specifies —
**not Node**, which the design rejects for concrete reasons: Node's module identity
is a file path but we need a content hash (C2); we need `Isolate` = one shared heap
(pure nodes shared across branches by reference, C4) and `Context` = one branch's
governed globals (C4, §13), which Node's single ambient realm cannot provide; and
governed effects mean *constructing* a Context's globals from scratch (§6.1).

The split, faithful to §4.1's trusted kernel:

- **Rust is the host kernel** — the raw-V8 substrate (Isolate/Contexts), the
  content-addressed definition store + namespace (§4), governed-effect Contexts (§6),
  the incremental engine (§3), the layered store + 0-copy branches (§7), and the
  boundary-adapter I/O (§9.9).
- **The running dataflow graph lives inside V8 as content-addressed JS** — Z-sets,
  the lens catalogue, the scheduler, and the incremental `<Table>` (§9, §11). Deltas
  are V8 objects, so lens transforms never marshal across an FFI boundary; Rust
  crosses into V8 only coarsely (per submit / per poll), at the adapter/host seam
  where serialization belongs.

## What runs (146 Rust tests: `cargo test`)

| Subsystem | §ref | Where | Status |
|---|---|---|---|
| Content-addressed definitions + namespace | §4, C2 | Rust | ✓ hashing + store (rename-free, versions retained) |
| Bare-V8 substrate, branch Contexts | §14 | Rust/V8 | ✓ one Isolate, many branch Contexts |
| Cross-Context heap sharing (V2) | §14 | Rust/V8 | ✓ pure node read by reference across branches |
| Hot-swap rebind→observe (V3) | §14, §5.1 | Rust/V8 | ✓ ~18µs live redefinition |
| Governed-effect Contexts | §6, §6.1 | Rust/V8 | ✓ seeded/deterministic/replayable; ambient deleted |
| Incremental engine, early cutoff | §3, C1 | Rust | ✓ blast-radius edits; 0 downstream on no-op |
| Persistent map (structural sharing) | §7, C7 | Rust | ✓ HAMT, path-copying |
| Layered store + 0-copy branches | §7, C7 | Rust | ✓ tombstones, fork, promote, diff |
| Z-set delta algebra | §9.7, C5 | V8/JS | ✓ signed multiplicities, invertible |
| Bidirectional lenses (k1–k4) | §9.5 | V8/JS | ✓ calibrate/filter/map/join/count/topk |
| Scheduler (early cutoff, echo, outbox) | §9.6 | V8/JS | ✓ glitch-free topo order; join bilinear fix |
| Boundary adapter (poll+diff, write-through) | §9.9 | Rust | ✓ manufactured edit stream; echo-suppressed |
| Incremental `<Table>` (virtualized) | §11.7 | V8/JS | ✓ minimal mutations, stable keys |
| Schema inference (profile + annotate) | §12.3 | V8/JS | ✓ types, keys, references, enums, patterns |
| Industrial format readers | §9.9, §8.1 | Rust | ✓ WITSML/EDM XML, LAS, xlsx, fixed-width, zip+inflate |
| Documents: PDF text → OCR → search | §9.8, §9.9 | Rust | ✓ text layer, vision-model OCR ladder, local embeddings |
| **§18 first-milestone loop, end-to-end** | §18 | Rust+V8 | ✓ DB→lenses→join→Table + backward round-trip |

## Run it

```bash
# one-time: install Rust — curl https://sh.rustup.rs -sSf | sh
cargo run                              # §4/§6/§14 substrate demo (content-addr, branching, governed)
cargo run --bin serve                  # the twin: agent workspace at http://127.0.0.1:8080
cargo run --example milestone          # the §18 loop end-to-end on Rust+V8
cargo run --example bench --release    # §17 invariants (A1, overlay, hot-swap, effects, table)
cargo test                             # 45 tests
```

Measured here (`--release`): A1 edit recomputes ~4% of a 2060-node graph, **0**
downstream on a no-op output; **200,000** 0-copy branches at **~0.13 µs/fork**;
hot-swap **~18 µs**; governed branches byte-identical under the same seed; table
median **0** mutations/edit, **1** per visible change.

## Layout

```
design_doc.md   the specification — section references (§) throughout the code point here
src/            the Rust kernel: content addressing, the raw-V8 host, the incremental
                engine, layered store + branches, boundary adapters (local files,
                external APIs), the serve server, and the agent harness
src/js/         the dataflow graph + twin app, as content-addressed JS running in V8
web/            the thin client (vanilla JS: apply mutations, send events back)
skills/         agent skills, discovered at startup
data/           mounted project data (Valhall snapshot, Volve drilling samples, AIS)
                + journal.jsonl, the event log
examples/       the §18 milestone loop and the §17 benchmarks
tests/          integration tests (substrate, dataflow, agent perception, schema, protobuf)
```
