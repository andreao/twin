#!/usr/bin/env bash
# obtain-volve — pull Equinor's open Volve drilling corpus into the twin.
#
# The full dataset is registration-gated (see SKILL.md), so `pull` fetches from
# VOLVE_BASE: any HTTP mirror or a locally served extract of the official archive.
# The bundled samples under data/volve/ already cover every format offline.
set -euo pipefail

OUT="${VOLVE_OUT:-data/volve}"
BASE="${VOLVE_BASE:-}"
MANIFEST="${VOLVE_MANIFEST:-manifest.txt}"

status() {
  echo "volve data under $OUT:"
  for d in witsml edm logs picks files; do
    n=$(find "$OUT/$d" -type f 2>/dev/null | wc -l | tr -d ' ')
    echo "  $d: $n file(s)"
  done
}

sample() {
  # the samples ship with the repo — just prove they parse-worthy exist
  local missing=0
  for f in witsml/drillReports.xml witsml/trajectory.xml edm/export.xml \
           logs/15_9-F-14.las picks/wellpicks.txt files/final-well-report-15-9-F-14.pdf; do
    if [ ! -f "$OUT/$f" ]; then echo "MISSING: $OUT/$f"; missing=1; fi
  done
  [ "$missing" = 0 ] && echo "all bundled samples present — mount them with read_source"
  status
}

pull() {
  if [ -z "$BASE" ]; then
    echo "VOLVE_BASE is not set." >&2
    echo "Register at https://www.equinor.com/energy/volve-data-sharing, download the" >&2
    echo "drilling subset, then either point VOLVE_BASE at a mirror or serve your" >&2
    echo "extract locally:  cd <extract> && python3 -m http.server 8000" >&2
    echo "                  VOLVE_BASE=http://127.0.0.1:8000 $0 pull" >&2
    exit 2
  fi
  # the manifest is one relative path per line, e.g. witsml/15_9-F-14/drillReports.xml
  echo "fetching $BASE/$MANIFEST"
  local list
  list=$(curl -fsS --max-time 60 "$BASE/$MANIFEST")
  local n=0
  while IFS= read -r rel; do
    [ -z "$rel" ] && continue
    case "$rel" in \#*) continue ;; esac
    mkdir -p "$OUT/$(dirname "$rel")"
    echo "  $rel"
    curl -fsS --max-time 300 "$BASE/$rel" -o "$OUT/$rel"
    n=$((n + 1))
  done <<< "$list"
  echo "pulled $n file(s) into $OUT"
  status
}

case "${1:-sample}" in
  sample) sample ;;
  pull) pull ;;
  status) status ;;
  *) echo "usage: $0 [sample|pull|status]" >&2; exit 2 ;;
esac
