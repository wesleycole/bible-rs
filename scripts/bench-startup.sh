#!/usr/bin/env bash
# Measures startup (process launch → first painted frame) and scrolling frame times.
#
#   scripts/bench-startup.sh          # 10 launches + one scroll run
#   RUNS=20 scripts/bench-startup.sh
#   LINGER_RENDERER=wgpu scripts/bench-startup.sh   # glow (default) or wgpu, app and baseline
#   SYNC=1 scripts/bench-startup.sh   # also, alternating, signed in with library sync on against
#                                     # the backend server/scripts/library-local.sh started; each
#                                     # sync run must pull a write made just before it, or it fails
#
# On Linux without a display it starts Xvfb (software OpenGL); on macOS it opens real windows.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/linger}"
FLOOR="$ROOT/target/release/examples/startup_floor"
RUNS="${RUNS:-10}"
[[ -x "$BIN" ]] || (cd "$ROOT" && cargo build --release -p linger)
[[ -x "$FLOOR" ]] || (cd "$ROOT" && cargo build --release -p linger --example startup_floor)
if [[ "$(uname -s)" == "Linux" ]]; then
  export DISPLAY="${DISPLAY:-:98}"
  export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
  if ! xdpyinfo >/dev/null 2>&1; then
    Xvfb "$DISPLAY" -screen 0 1600x1000x24 >/dev/null 2>&1 &
    XVFB_PID=$!
    trap 'kill $XVFB_PID 2>/dev/null || true' EXIT
    sleep 1
  fi
fi
HOME_DIR="$(mktemp -d)"
# One warm-up launch creates the database (first run only) and warms the OS file cache.
LINGER_HOME="$HOME_DIR" LINGER_STARTUP_PROBE=1 "$BIN" >/dev/null 2>&1 || true
SYNC_HOME=""
if [[ "${SYNC:-}" == 1 ]]; then
  : "${LINGER_TEST_CONVEX_URL:?SYNC=1 needs server/scripts/library-local.sh}" "${LINGER_TEST_TOKEN:?}"
  SYNC_HOME="$(mktemp -d)"
  LINGER_HOME="$SYNC_HOME" LINGER_STARTUP_PROBE=1 "$BIN" >/dev/null 2>&1 || true
  # The library consent sheet accepted, so the engine starts with the window.
  python3 - "$SYNC_HOME/study.sqlite" <<'PY'
import json, sqlite3, sys
db = sqlite3.connect(sys.argv[1])
row = db.execute("SELECT value FROM settings WHERE key = 'app'").fetchone()
app = json.loads(row[0]) if row else {}
app["library_consent"] = True
db.execute("INSERT INTO settings(key, value) VALUES('app', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", (json.dumps(app),))
db.commit()
PY
fi
# Signed in with sync on (only for the SYNC=1 home's runs).
sync_env() {
  LINGER_API_TOKEN="$LINGER_TEST_TOKEN" LINGER_CONVEX_URL="$LINGER_TEST_CONVEX_URL" \
    LINGER_KEYCHAIN_SERVICE="${LINGER_KEYCHAIN_SERVICE:-linger-bench}" "$@"
}
# Another device's write, straight to the backend: prints its seq, which the next run must pull.
remote_write() {
  curl -fsS "$LINGER_TEST_CONVEX_URL/api/mutation" -H "Authorization: Bearer $LINGER_TEST_TOKEN" \
    -H 'Content-Type: application/json' \
    -d "{\"path\":\"library/highlights:set\",\"args\":{\"verse\":$((19023001 + $1 % 6)),\"color\":\"$([[ $(($1 % 2)) == 0 ]] && echo blue || echo green)\"},\"format\":\"json\"}" |
    python3 -c 'import json,sys; print(int(json.load(sys.stdin)["value"]["record"]["seq"]))'
}

now_ms() { python3 -c 'import time; print(time.time()*1000)'; }
field() { python3 -c 'import json,sys; print(json.loads(sys.argv[1])[sys.argv[2]])' "$1" "$2"; }
built=() presented=() external=() floor_built=() floor_presented=()
sbuilt=() spresented=() sexternal=() synced=()
# One launch: appends its built / presented / wall times to the arrays named by $1 (a prefix).
measure() {
  local prefix=$1 home=$2 out start line
  shift 2
  start=$(now_ms)
  out=$(LINGER_HOME="$home" LINGER_STARTUP_PROBE=1 "$@" "$BIN" 2>/dev/null)
  line=$(grep first_frame_built_ms <<<"$out" | tail -1)
  eval "${prefix}built+=(\"$(field "$line" first_frame_built_ms)\")"
  eval "${prefix}presented+=(\"$(field "$line" first_frame_presented_ms)\")"
  eval "${prefix}external+=(\"$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["epoch_ms"] - float(sys.argv[2]))' "$line" "$start")\")"
  LAST_SYNC=$(grep sync_ms <<<"$out" | tail -1 || true)
}
for i in $(seq 1 "$RUNS"); do
  # Baseline: an empty eframe window with the same options (the platform's floor).
  fl=$("$FLOOR" 2>/dev/null | tail -1)
  floor_built+=("$(field "$fl" first_frame_built_ms)")
  floor_presented+=("$(field "$fl" first_frame_presented_ms)")
  # With SYNC=1 the two kinds of launch alternate, and which goes first alternates too, so drift
  # in the machine's speed and the second launch's warmer caches fall on both alike.
  (( i % 2 == 0 )) || [[ -z "$SYNC_HOME" ]] && measure "" "$HOME_DIR" env
  if [[ -n "$SYNC_HOME" ]]; then
    want=$(remote_write "$i")
    measure s "$SYNC_HOME" sync_env
    got=$(python3 -c 'import json,sys; v=json.loads(sys.argv[1] or "{}"); print(int(v.get("cursor") or 0))' "$LAST_SYNC")
    if ! [[ "$got" =~ ^[0-9]+$ && "$want" =~ ^[0-9]+$ ]] || (( got < want )); then
      echo "run $i: sync didn't pull seq $want (the engine reported ${LAST_SYNC:-nothing})" >&2
      exit 1
    fi
    synced+=("$(field "$LAST_SYNC" sync_ms)")
    (( i % 2 == 1 )) && measure "" "$HOME_DIR" env
  fi
done
python3 - "${built[*]}" "${presented[*]}" "${external[*]}" "${floor_built[*]}" "${floor_presented[*]}" \
  "${sbuilt[*]:-}" "${spresented[*]:-}" "${sexternal[*]:-}" "${synced[*]:-}" <<'PY'
import statistics, sys
names = [
    "linger: first frame built",
    "linger: first frame presented",
    "linger: launch → first frame presented (wall)",
    "empty eframe window: first frame built",
    "empty eframe window: first frame presented",
    "linger, sync on: first frame built",
    "linger, sync on: first frame presented",
    "linger, sync on: launch → first frame presented (wall)",
    "sync on: first frame → caught up",
]
med = {}
for name, arg in zip(names, sys.argv[1:]):
    if not arg.strip():
        continue
    v = sorted(float(x) for x in arg.split())
    med[name] = statistics.median(v)
    print(f"{name:<56} median {med[name]:7.1f} ms   min {v[0]:7.1f}   max {v[-1]:7.1f}   (n={len(v)})")
for off, on in zip(names[:3], names[5:8]):
    if on in med:
        print(f"sync on vs off, {off.split(': ', 1)[1]:<44} {100 * (med[on] / med[off] - 1):+6.1f}%")
PY
echo "renderer: ${LINGER_RENDERER:-default}, sync: ${SYNC:-off}"
echo "scrolling (900 frames, ~2,200 px/s through consecutive chapters):"
LINGER_HOME="$HOME_DIR" LINGER_SCROLL_PROBE=1 "$BIN" 2>/dev/null | tail -1
rm -rf "$HOME_DIR" ${SYNC_HOME:+"$SYNC_HOME"}
