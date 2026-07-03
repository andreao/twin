# An Incremental Runtime

A working implementation of the digital-twin substrate in [`design_doc.md`](./design_doc.md):
*one local-first incremental computation graph that anyone can pour data into, that
derives and connects, that any number of use-cases sit on.*

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

## What runs (31 Rust tests: `cargo test`)

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
| **§18 first-milestone loop, end-to-end** | §18 | Rust+V8 | ✓ DB→lenses→join→Table + backward round-trip |

## Run it

```bash
# one-time: install Rust — curl https://sh.rustup.rs -sSf | sh
cargo run                              # §4/§6/§14 substrate demo (content-addr, branching, governed)
cargo run --example milestone          # the §18 loop end-to-end on Rust+V8
cargo run --example bench --release    # §17 invariants (A1, overlay, hot-swap, effects, table)
cargo test                             # 31 tests
```

Measured here (`--release`): A1 edit recomputes ~4% of a 2060-node graph, **0**
downstream on a no-op output; **200,000** 0-copy branches at **~0.13 µs/fork**;
hot-swap **~18 µs**; governed branches byte-identical under the same seed; table
median **0** mutations/edit, **1** per visible change.

## Layout

```
design_doc.md              the specification
Cargo.toml
src/
  hashing.rs definitions.rs   content addressing + namespace (§4, C2)
  runtime.rs                  raw-V8 host: branches, governed effects, hot-swap (§6, §14)
  engine.rs                   incremental engine, early cutoff (§3, C1)
  pmap.rs layers.rs           persistent map + layered store + 0-copy branches (§7, C7)
  adapter.rs                  boundary adapter: mock DB + poll/diff + write-through (§9.9)
  jsgraph.rs                  Rust host that drives the V8-resident dataflow graph
  js/                         the dataflow graph, as content-addressed JS running in V8
    zset.js graph.js table.js milestone.js   Z-sets, lenses+scheduler, table, §18 app
examples/  milestone.rs bench.rs
tests/     substrate.rs dataflow.rs milestone.rs   (+ unit tests in each src module)
```
