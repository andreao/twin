#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# pull_oid.sh — extract Cognite Open Industrial Data (OID / Aker BP Valhall)
#
# A twin-agent skill: mounts a real, multi-modal industrial corpus (asset
# hierarchy, sensor time-series, P&ID documents, maintenance events, 3D model
# mappings) from Cognite's public `publicdata` project into flat files the twin
# can ingest.  Pure bash + curl + jq — no Python, no SDK.
#
# USAGE (run from the repo root):
#   ./skills/obtain-oid/pull_oid.sh login        # one-time OIDC device-code sign-in
#   ./skills/obtain-oid/pull_oid.sh refresh       # swap refresh_token -> fresh access token
#   ./skills/obtain-oid/pull_oid.sh pull          # pull everything into $OID_OUT
#   ./skills/obtain-oid/pull_oid.sh datapoints    # (or any single stage)
#
# ENV (all optional):
#   OID_TOKEN_FILE  token json path            (default /tmp/cognite_token.json)
#   OID_OUT         output dir                 (default data/cognite)
#   OID_DAYS        raw datapoint window, days (default 30)
#   OID_EVENTS_CAP  max events to pull         (default 100000; full set is 39.8M)
# ---------------------------------------------------------------------------
set -uo pipefail

TENANT=48d5043c-cf70-4c49-881c-c638f5796997
CLIENT=1b90ede3-271e-401b-81a0-a4d52bea3273        # "OID-Api" — accepts personal MS accounts
SCOPE="https://api.cognitedata.com/user_impersonation offline_access openid profile"
BASE="https://api.cognitedata.com/api/v1/projects/publicdata"
AUTH="https://login.microsoftonline.com/$TENANT/oauth2/v2.0"

TOKEN_FILE="${OID_TOKEN_FILE:-/tmp/cognite_token.json}"
OUT="${OID_OUT:-data/cognite}"
DAYS="${OID_DAYS:-30}"
EVENTS_CAP="${OID_EVENTS_CAP:-100000}"
FORMAT="${OID_FORMAT:-json}"     # json (decode -> CSV) | protobuf (raw .pb, decode deferred)
PAR="${OID_PAR:-8}"              # datapoints fan-out width
SERIES_LIMIT="${OID_SERIES_LIMIT:-100000}"  # cap #series (for quick demos)

# absolute self-path + output dir, exported so parallel `_series` children inherit them
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT";; esac
export OID_OUT="$OUT" OID_DAYS="$DAYS" OID_TOKEN_FILE="$TOKEN_FILE" OID_FORMAT="$FORMAT"

tok(){ jq -r .access_token "$TOKEN_FILE"; }
# every CDF call: bearer + json; responses are run through tr -d '[:cntrl:]' by callers
# because some metadata fields contain raw (unescaped) control characters.
api(){ curl -s --max-time 90 -H "Authorization: Bearer $(tok)" -H "Content-Type: application/json" "$@"; }

# ---- auth -----------------------------------------------------------------
cmd_login(){
  local resp; resp=$(curl -s "$AUTH/devicecode" -d "client_id=$CLIENT" --data-urlencode "scope=$SCOPE")
  echo "$resp" | jq -r .message
  local dc; dc=$(echo "$resp" | jq -r .device_code)
  printf 'After approving in the browser, press Enter to continue… '; read -r _
  local r; r=$(curl -s "$AUTH/token" -d "client_id=$CLIENT" \
    --data-urlencode "grant_type=urn:ietf:params:oauth:grant-type:device_code" \
    --data-urlencode "device_code=$dc")
  if echo "$r" | jq -e .access_token >/dev/null 2>&1; then
    echo "$r" > "$TOKEN_FILE"; chmod 600 "$TOKEN_FILE"; echo "token saved -> $TOKEN_FILE"
  else echo "login failed: $(echo "$r" | jq -r '.error_description' | head -1)"; return 1; fi
}
cmd_refresh(){
  local rt; rt=$(jq -r '.refresh_token // empty' "$TOKEN_FILE")
  [ -z "$rt" ] && { echo "no refresh_token; run 'login'"; return 1; }
  local r; r=$(curl -s "$AUTH/token" -d "client_id=$CLIENT" -d "grant_type=refresh_token" \
    --data-urlencode "refresh_token=$rt" --data-urlencode "scope=$SCOPE")
  if echo "$r" | jq -e .access_token >/dev/null 2>&1; then
    echo "$r" > "$TOKEN_FILE"; chmod 600 "$TOKEN_FILE"; echo "refreshed ($(echo "$r"|jq -r .expires_in)s)"
  else echo "refresh failed: $(echo "$r" | jq -r '.error')"; return 1; fi
}

# ---- data stages ----------------------------------------------------------
cmd_assets(){
  mkdir -p "$OUT"; : > "$OUT/_a.ndjson"; local cur=""
  while :; do
    local b; b=$([ -z "$cur" ] && echo '{"limit":1000}' || printf '{"limit":1000,"cursor":"%s"}' "$cur")
    api -d "$b" "$BASE/assets/list" | tr -d '[:cntrl:]' > "$OUT/_p.json"
    jq -c '.items[]' "$OUT/_p.json" >> "$OUT/_a.ndjson"
    cur=$(jq -r '.nextCursor // empty' "$OUT/_p.json"); [ -z "$cur" ] && break
  done
  jq -s '.' "$OUT/_a.ndjson" > "$OUT/assets.json"; rm -f "$OUT/_a.ndjson" "$OUT/_p.json"
  jq -r '["id","parentId","name","description"],(.[]|[.id,.parentId,.name,.description])|@csv' \
    "$OUT/assets.json" > "$OUT/assets.csv"
  echo "[assets] $(jq length "$OUT/assets.json") -> assets.{json,csv}"
}
cmd_timeseries(){
  api -d '{"limit":1000}' "$BASE/timeseries/list" | tr -d '[:cntrl:]' | jq '.items' > "$OUT/timeseries.json"
  jq -r '["id","externalId","name","unit","isString","assetId","description"],(.[]|[.id,.externalId,.name,.unit,.isString,.assetId,.description])|@csv' \
    "$OUT/timeseries.json" > "$OUT/timeseries.csv"
  echo "[timeseries] $(jq length "$OUT/timeseries.json") -> timeseries.{json,csv}"
}
cmd_files(){
  mkdir -p "$OUT/files"
  api -d '{"limit":1000}' "$BASE/files/list" | tr -d '[:cntrl:]' | jq '.items' > "$OUT/files.json"
  jq -r '.[]|[.id,.name]|@tsv' "$OUT/files.json" | while IFS=$'\t' read -r id name; do
    local url; url=$(api -d "{\"items\":[{\"id\":$id}]}" "$BASE/files/downloadlink" | jq -r '.items[0].downloadUrl')
    curl -s --max-time 180 -o "$OUT/files/$name" "$url" && echo "  [file] $name"
  done
  echo "[files] $(ls "$OUT/files" | wc -l | tr -d ' ') downloaded"
}
cmd_events(){
  echo "id,type,subtype,startTime,endTime,description,assetIds" > "$OUT/events.csv"
  local cur="" got=0
  while [ "$got" -lt "$EVENTS_CAP" ]; do
    local b; b=$([ -z "$cur" ] && echo '{"limit":1000}' || printf '{"limit":1000,"cursor":"%s"}' "$cur")
    local r; r=$(api -d "$b" "$BASE/events/list" | tr -d '[:cntrl:]')
    echo "$r" | jq -r '.items[]|[.id,.type,.subtype,.startTime,.endTime,.description,((.assetIds//[])|join(";"))]|@csv' >> "$OUT/events.csv"
    cur=$(echo "$r" | jq -r '.nextCursor // empty'); got=$((got+1000))
    [ -z "$cur" ] && break
  done
  echo "[events] $(( $(wc -l < "$OUT/events.csv") - 1 )) rows (of 39.8M total)"
}
# pull ONE series (internal; invoked in parallel by cmd_datapoints via xargs).
# Two ingestion strategies, selected by $OID_FORMAT — the same logical datapoints,
# demonstrating both cases the twin supports (decoded-eager vs raw-lazy):
#   json     — decode on the wire, paginate by last-timestamp -> datapoints/<id>.csv
#   protobuf — request protobuf, dump raw bytes per fixed daily window (no decode,
#              so no timestamp-pagination and no protoc needed) -> datapoints_pb/<id>/DD.pb
cmd_series(){
  local id="$1"
  if [ "$FORMAT" = "protobuf" ]; then
    local dir="$OUT/datapoints_pb/$id"; mkdir -p "$dir"; local d=0
    while [ "$d" -lt "$DAYS" ]; do
      local ws=$(( OID_START + d*86400000 )) we=$(( OID_START + (d+1)*86400000 ))
      curl -s --max-time 60 -H "Authorization: Bearer $(tok)" -H "Content-Type: application/json" \
        -H "Accept: application/protobuf" \
        -d "{\"items\":[{\"id\":$id}],\"start\":$ws,\"end\":$we,\"limit\":100000}" \
        "$BASE/timeseries/data/list" -o "$dir/$(printf '%02d' "$d").pb"
      d=$((d+1))
    done
  else
    local out="$OUT/datapoints/$id.csv"; echo "timestamp,value" > "$out"
    local s="$OID_START" page=0
    while [ "$page" -lt 80 ]; do
      local r n; r=$(api -d "{\"items\":[{\"id\":$id}],\"start\":$s,\"end\":$OID_END,\"limit\":100000}" "$BASE/timeseries/data/list")
      n=$(echo "$r" | jq '.items[0].datapoints|length // 0')
      [ "${n:-0}" -eq 0 ] && break
      echo "$r" | jq -r '.items[0].datapoints[]|[.timestamp,.value]|@csv' >> "$out"
      s=$(( $(echo "$r" | jq '.items[0].datapoints[-1].timestamp') + 1 )); page=$((page+1))
      [ "$n" -lt 100000 ] && break
    done
  fi
}

cmd_datapoints(){
  [ -f "$OUT/timeseries.json" ] || cmd_timeseries
  # anchor the window at the newest datapoint across a numeric series (once, shared)
  local nid; nid=$(jq -r '[.[]|select(.isString==false)][0].id' "$OUT/timeseries.json")
  OID_END=$(api -d "{\"items\":[{\"id\":$nid}]}" "$BASE/timeseries/data/latest" | jq '.items[0].datapoints[0].timestamp')
  OID_START=$(( OID_END - DAYS*86400000 ))
  export OID_START OID_END
  mkdir -p "$OUT/datapoints" "$OUT/datapoints_pb"
  local ids count; ids=$(jq -r '.[].id' "$OUT/timeseries.json" | head -n "$SERIES_LIMIT")
  count=$(echo "$ids" | wc -l | tr -d ' ')
  echo "[datapoints] ${DAYS}d raw, format=$FORMAT, $count series, ${PAR}-way parallel…"
  echo "$ids" | xargs -P "$PAR" -I{} "$SELF" _series {}
  if [ "$FORMAT" = "protobuf" ]; then
    echo "[datapoints] protobuf: $(find "$OUT/datapoints_pb" -name '*.pb' 2>/dev/null | wc -l | tr -d ' ') .pb, $(du -sh "$OUT/datapoints_pb" 2>/dev/null | cut -f1)"
  else
    echo "[datapoints] json: $(find "$OUT/datapoints" -name '*.csv' 2>/dev/null | wc -l | tr -d ' ') series, $(du -sh "$OUT/datapoints" 2>/dev/null | cut -f1)"
  fi
}
cmd_3d(){
  mkdir -p "$OUT/3d"
  api "$BASE/3d/models?limit=100" | jq '.items' > "$OUT/3d/models.json"
  jq -r '.[].id' "$OUT/3d/models.json" | while read -r m; do
    api "$BASE/3d/models/$m/revisions" | jq '.items' > "$OUT/3d/revisions_$m.json"
    local rev; rev=$(jq -r '[.[]|select(.status=="Done")][0].id // empty' "$OUT/3d/revisions_$m.json")
    [ -z "$rev" ] && continue
    api "$BASE/3d/models/$m/revisions/$rev/mappings?limit=1000" | jq '.items' > "$OUT/3d/mappings_${m}_${rev}.json"
  done
  echo "[3d] models + revisions + node->asset mappings saved"
}

cmd_pull(){ mkdir -p "$OUT"; cmd_assets; cmd_timeseries; cmd_files; cmd_3d; cmd_events; cmd_datapoints; echo "[pull] complete -> $OUT ($(du -sh "$OUT" | cut -f1))"; }

case "${1:-pull}" in
  login) cmd_login;; refresh) cmd_refresh;;
  assets) cmd_assets;; timeseries) cmd_timeseries;; files) cmd_files;;
  events) cmd_events;; datapoints) cmd_datapoints;; 3d) cmd_3d;;
  _series) cmd_series "${2:-}";;   # internal: one series, run in parallel by datapoints
  pull) cmd_pull;;
  *) echo "usage: $0 {login|refresh|pull|assets|timeseries|files|events|datapoints|3d}"; exit 1;;
esac
