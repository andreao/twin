// The §18 first-milestone graph, built in V8 (design_doc §18).
//
// DB adapters (Rust) -> calibrate -> filter -> join -> <Table>.  This file builds
// the JS dataflow graph and exposes a tiny API the Rust host drives: submit deltas
// manufactured by the Rust poll+diff adapter, push a UI cell edit backward, drain
// the write-through outbox, and read the rendered table.  Deltas cross the Rust<->JS
// boundary as JSON (the §9.9 adapter boundary); inside V8 they are native objects.

'use strict';

const M = (() => {
  const G = new Graph('main');
  const streamIds = {};

  // external sources (backed by the Rust boundary adapters, §9.9)
  streamIds.assets = G.source('assets', true);
  streamIds.readings = G.source('readings', true);

  // pure lenses
  const cal = G.apply(calibrateLens('temp', 2.0), [streamIds.readings], 'calibrate');
  const warm = G.apply(filterLens(r => r.get('temp') >= 40.0), [cal], 'warm');
  // join asset metadata x calibrated readings on id == asset
  const joinId = G.apply(
    joinLens('id', 'asset', ['name', 'site'], ['temp'], ''),
    [streamIds.assets, warm], 'join');

  // incremental table observing the join
  const table = new Table(['id', 'name', 'site', 'temp'],
    { idField: 'id', sortField: 'temp', descending: true, window: [0, 10] });
  G.observe(joinId, (delta) => { G.hostMutations += table.forward(delta); });

  return {
    submit(stream, deltaJSON, prov) {
      const evs = G.submit(streamIds[stream], ZSet.fromJSON(deltaJSON), prov);
      return JSON.stringify(evs);
    },
    cellEdit(id, col, value, prov) {
      G.pushBackward(joinId, table.cellEditToDelta(id, col, value), prov);
      return JSON.stringify(G.drainOutbox());
    },
    drainOutbox() { return JSON.stringify(G.drainOutbox()); },
    render() { return JSON.stringify(table.render()); },
    visibleRows() { return JSON.stringify(table.visibleRows()); },
    mutationCount() { return G.hostMutations; },
    blame(stream) {
      return JSON.stringify(G.log.filter(e => e.stream === stream)
        .map(e => ({ seq: e.seq, ts: e.ts, author: e.provenance.author, origin: e.provenance.origin, note: e.note })));
    },
  };
})();
globalThis.M = M;
