---
name: obtain-volve
title: Obtain Volve drilling data
description: Pull Equinor's open Volve field drilling corpus into the twin — WITSML daily drill reports and trajectories, EDM engineering exports, LAS well logs, formation picks and final well reports. Bundled samples work offline; the full dataset needs a one-time registration.
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
./skills/obtain-volve/pull_volve.sh pull       # fetch from a Volve mirror into data/volve/
./skills/obtain-volve/pull_volve.sh status     # what's on disk right now
```

Tunables via env: `VOLVE_OUT` (output dir, default `data/volve`), `VOLVE_BASE`
(HTTP base of a Volve mirror or your own extract of the official archive),
`VOLVE_MANIFEST` (path list to fetch relative to the base, default `manifest.txt`
at the base).

## Access — the honest part

The full Volve dataset (~5 TB with all real-time WITSML; a few GB for the useful
drilling subset) is free but **registration-gated**: accept the license at
https://www.equinor.com/energy/volve-data-sharing and you get a download link.
There is no stable anonymous endpoint, so `pull` needs `VOLVE_BASE` pointed at
either a mirror you have access to or a directory you extracted from the official
archive and serve yourself (`python3 -m http.server` over the extract works).

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
