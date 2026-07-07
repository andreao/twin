---
name: obtain-oid
title: Obtain OID
description: Pull Cognite Open Industrial Data (Aker BP Valhall) into the twin — real O&G assets, sensor time-series, P&ID documents, events, and 3D. Commands — login (one-time human device-code sign-in; the code to give the user appears in the output), refresh (new access token), pull (everything), or one stage of assets|timeseries|files|events|datapoints|3d.
tool: pull_oid.sh
---

# Skill: obtain-oid

**Owner:** the twin agent (this is a twin skill, not a Claude Code skill).
**Purpose:** pull a real, multi-modal industrial corpus into the twin — Cognite's
public **Open Industrial Data (OID)**: sensor/compressor data from Aker BP's
**Valhall** oil platform. Use it to seed or refresh the twin with genuine
oil-&-gas operational data across many kinds.

## What it gives the twin

| Kind | Count | File(s) |
|---|---|---|
| Asset hierarchy (equipment inventory) | 1,115 | `assets.{json,csv}` |
| Time-series (sensors) — metadata | 445 | `timeseries.{json,csv}` |
| Time-series — **raw datapoints** | ~7,940 pts/sensor/day | `datapoints/<id>.csv` |
| Documents (P&IDs: PDF + processed SVG, + a video) | 19 | `files/` |
| Maintenance events | 39.8M total | `events.csv` (bounded sample) |
| 3D models + node→asset mappings | 3 | `3d/` |

All land under `data/cognite/` (gitignored) as flat CSV/JSON/files the twin mounts.

## Tooling

`pull_oid.sh` — pure bash + `curl` + `jq` (no Python, no SDK). Subcommands:

```
./skills/obtain-oid/pull_oid.sh login       # OIDC device-code sign-in (interactive, once)
./skills/obtain-oid/pull_oid.sh refresh      # refresh the access token (valid ~1h)
./skills/obtain-oid/pull_oid.sh pull         # pull everything into data/cognite/
./skills/obtain-oid/pull_oid.sh datapoints   # or run one stage: assets|timeseries|files|events|datapoints|3d
```

Tunables via env: `OID_OUT` (output dir), `OID_DAYS` (raw datapoint window, default 30),
`OID_EVENTS_CAP` (default 100k), `OID_TOKEN_FILE` (default `/tmp/cognite_token.json`),
`OID_PAR` (datapoint fan-out width, default 8), `OID_SERIES_LIMIT` (cap #sensors for demos),
and **`OID_FORMAT`** — the two ingestion strategies:
- `json` (default) — decode on the wire, paginate by timestamp → `datapoints/<id>.csv`.
  Eager, immediately queryable; CPU-bound on `jq` (the ceiling; parallelism gives ~2× here).
- `protobuf` — request protobuf, dump raw bytes per fixed **daily** window (no in-payload
  decode → no `protoc`, no timestamp-pagination, no truncation) → `datapoints_pb/<id>/DD.pb`.
  ~2× smaller on disk, decode deferred to read-time (needs the CDF datapoints proto later).

## Access — the one non-obvious part

OID sits in Cognite's Azure AD tenant behind **two** apps:
- **Cognite Hub** (SAML) — rejects a personal Microsoft/Google account with `AADSTS50020`.
  Do **not** use this. It's the wall that makes people think OID is inaccessible.
- **OID-Api** (client `1b90ede3-…`) — **accepts a personal account** via the OIDC
  **device-code flow**. This is what `login` uses. No Cognite account, no client secret.

Public credentials (from the [OID OpenID-Connect page](https://hub.cognite.com/open-industrial-data-211/openid-connect-on-open-industrial-data-993)):
`tenant=48d5043c-cf70-4c49-881c-c638f5796997`, `client=1b90ede3-271e-401b-81a0-a4d52bea3273`,
`scope=https://api.cognitedata.com/user_impersonation`, `project=publicdata`,
`cluster=api` (base `https://api.cognitedata.com`).

Gotchas: the device code expires in 15 min (re-run `login`); the access token lasts ~1h
(`refresh` uses the `offline_access` refresh token); full raw datapoint history is ~10^10
points, so `datapoints` pulls a bounded `OID_DAYS` window; events is 39.8M rows, so it's
capped. The token file is a secret — mode 600, never commit.

## When the agent should use this

When the twin needs real O&G operational data — to demo, to build lenses/deriveds over
sensors + assets + documents, or to refresh a stale pull. If the token is missing/expired,
run `login` (needs the user to approve in a browser once), else `refresh`, then `pull`.
