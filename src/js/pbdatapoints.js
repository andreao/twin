// The datapoints decode lens (design_doc §9.4) — PURE JS, runs in V8, no deps.
//
// Decodes Cognite CDF `DataPointListResponse` protobuf (numeric datapoints) into
// rows [[timestampMs, value], ...].  This is the "decode" half of the fetch→decode
// chain: the boundary adapter pulls raw protobuf bytes via a governed ctx.http
// capability; THIS lens — pure, deterministic, no protoc, no filesystem — turns those
// bytes into rows.  Protobuf's wire format is small enough to read directly:
//   wiretype 0 = varint, 1 = fixed64 (little-endian double), 2 = length-delimited.

'use strict';

function hexToBytes(hex) {
  const n = hex.length >> 1, out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

// Parse the fields of a protobuf message in [start,end) → [{field, wire, v}], where
// v is a BigInt (varint), {at} (fixed64/32 offset), or {s,e} (length-delimited range).
function pbFields(buf, start, end) {
  const out = [];
  let i = start;
  const varint = () => {
    let r = 0n, sh = 0n, b;
    do { b = buf[i++]; r |= BigInt(b & 0x7f) << sh; sh += 7n; } while (b & 0x80);
    return r;
  };
  while (i < end) {
    const tag = Number(varint());
    const field = tag >> 3, wire = tag & 7;
    if (wire === 0) out.push({ field, wire, v: varint() });
    else if (wire === 1) { out.push({ field, wire, v: { at: i } }); i += 8; }
    else if (wire === 2) { const len = Number(varint()); out.push({ field, wire, v: { s: i, e: i + len } }); i += len; }
    else if (wire === 5) { out.push({ field, wire, v: { at: i } }); i += 4; }
    else throw new Error('bad wiretype ' + wire);
  }
  return out;
}

function readDouble(buf, at) {
  return new DataView(buf.buffer, buf.byteOffset + at, 8).getFloat64(0, true);
}

// NumericDatapoints = repeated NumericDatapoint (field 1); each NumericDatapoint is
// { timestamp: field 1 varint, value: field 2 fixed64 double }.
function extractNumeric(buf, s, e) {
  const dps = [];
  for (const f of pbFields(buf, s, e)) {
    if (f.field !== 1 || f.wire !== 2) continue;
    let ts = null, val = null;
    for (const g of pbFields(buf, f.v.s, f.v.e)) {
      if (g.field === 1 && g.wire === 0) ts = g.v;
      else if (g.field === 2 && g.wire === 1) val = readDouble(buf, g.v.at);
    }
    if (ts !== null && val !== null) dps.push([Number(ts), val]);
  }
  return dps;
}

// DataPointListResponse: items = field 1; each item carries a numericDatapoints nested
// message. We don't hardcode its field number — we try each length-delimited subfield
// and keep the one that parses as datapoints (others, e.g. the externalId string, throw
// or yield nothing and are skipped).
function decodeDatapoints(buf) {
  const rows = [];
  for (const item of pbFields(buf, 0, buf.length)) {
    if (item.field !== 1 || item.wire !== 2) continue;
    for (const f of pbFields(buf, item.v.s, item.v.e)) {
      if (f.wire !== 2) continue;
      let dps;
      try { dps = extractNumeric(buf, f.v.s, f.v.e); } catch (_) { continue; }
      for (const d of dps) rows.push(d);
    }
  }
  return rows;
}

globalThis.hexToBytes = hexToBytes;
globalThis.decodeDatapoints = decodeDatapoints;
