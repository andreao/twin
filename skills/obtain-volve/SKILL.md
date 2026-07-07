---
name: obtain-volve
title: Obtain Volve drilling data
description: Pull Equinor's open Volve field drilling corpus into the twin — WITSML drill reports, trajectories, EDM exports, LAS logs, picks, reports. Commands — github (REAL WITSML for three wells, ~200 MB, no registration), sample (verify the bundled offline samples), pull (a full-corpus mirror via VOLVE_BASE), status (what's on disk).
tool: pull_volve.sh
---

# Skill: obtain-volve

**Owner:** the twin agent (this is a twin skill, not a Claude Code skill).
**Purpose:** bring a real drilling corpus into the twin — Equinor's **Volve** open
dataset (North Sea, wells 15/9-F-*): the same multi-format integration problem the
industry actually has (WITSML XML + EDM exports + LAS curves + picks + PDF reports),
mounted as flat sources the linking lenses join deterministically (§8.1).

## What it gives the twin

| Kind | Format | Lands under |
|---|---|---|
| Daily drill reports (activities, depths, comments) | WITSML XML | `data/volve/witsml/` |
| Wellbore trajectories (surveys) | WITSML XML | `data/volve/witsml/` |
| Engineering exports (wells, wellbores, BHAs, datums) | EDM XML | `data/volve/edm/` |
| Well log curves (GR, RHOB, NPHI, RT along depth) | LAS 2.0 | `data/volve/logs/` |
| Formation picks (tops per wellbore) | fixed-width text | `data/volve/picks/` |
| Final well reports | PDF | `data/volve/files/` |

Every format mounts with `read_source` (the twin's readers flatten each to rows);
PDFs are read with `read_document` (text layer → else local-model OCR) and become
searchable.

## Tooling

`pull_volve.sh` — pure bash + `curl` (no SDK). Subcommands:

```
./skills/obtain-volve/pull_volve.sh sample     # verify the bundled offline samples (default)
./skills/obtain-volve/pull_volve.sh github     # REAL WITSML, 3 wells, ~200 MB, no registration
./skills/obtain-volve/pull_volve.sh pull       # fetch from a Volve mirror into data/volve/
./skills/obtain-volve/pull_volve.sh status     # what's on disk right now
```

`github` pulls the f0nzie/volve-drilling mirror: the original Statoil WITSML
tree for wells NO 15/9-F-4, F-7 and F-9 (trajectories, BHA runs, tubulars,
rig, geometry, messages, real-time logs — 312 files, ~780k rows, all of which
parse through the twin's readers). Mount a WHOLE WELL with one read_source on
its directory, e.g. `data/volve/real/witsml/Norway-Statoil-NO 15_$47$_9-F-4` —
every row lands tagged with `kind` (the WITSML object type) and `file` (its
lineage), and the extraction lenses carve it from there
(`rows.filter(r => r.kind === 'trajectory')`).

Tunables via env: `VOLVE_OUT` (output dir, default `data/volve`), `VOLVE_BASE`
(HTTP base of a Volve mirror or your own extract of the official archive),
`VOLVE_MANIFEST` (path list to fetch relative to the base, default `manifest.txt`
at the base).

## Access — the honest part

Three rungs, by effort:
1. **`github` — now, no registration.** Real WITSML for three wells via the
   f0nzie/volve-drilling mirror (Equinor open licence, attribution required).
2. **The full dataset (~40,000 files)** is free but **registration-gated**: as of
   2026 the official route is the **Databricks Marketplace** listing "Volve Data
   Village" linked from https://www.equinor.com/energy/volve-data-sharing (a free
   Databricks account + accepting the Equinor Open Data Licence gets you the
   share; their "how to get access" PDF on that page walks it).
3. **`pull` with `VOLVE_BASE`** for any mirror or your own extract of the official
   archive served locally (`python3 -m http.server` over the extract works).
   The 2.7 GB pre-parsed real-time drilling CSVs once at the University of
   Stavanger (~atunkiel) are currently offline; their author is reachable if
   that subset matters.

The **bundled samples** under `data/volve/` are small, hand-authored files in the
real formats (WITSML 1.4.1.1, LAS 2.0, EDM export, NPD-style picks, PDF) for well
NO 15/9-F-14 — enough to build and demo every lens offline; `pull` replaces them
with the real corpus when you have it.

## When the agent should use this

When the user works with drilling data or asks what the twin can do with wells:
mount the samples immediately (they are already on disk — `read_source` each file
under `data/volve/`), and mention that the full Volve corpus can be pulled once
the user registers. The linking moves that matter: normalizeWell joins the WITSML
`nameWell` (NO 15/9-F-14) to the LAS `well` (15/9-F-14) and the picks `Wellbore`;
inInterval joins log depths and BHA runs to formation intervals from picks.
