#!/usr/bin/env bash
# Measures startup (process launch → first painted frame) and scrolling frame times.
#
#   scripts/bench-startup.sh          # 10 launches + one scroll run
#   RUNS=20 scripts/bench-startup.sh
#   LINGER_RENDERER=wgpu scripts/bench-startup.sh   # glow (default) or wgpu, app and baseline
#   SYNC=1 scripts/bench-startup.sh   # signed in, library sync on, against the backend
#                                     # server/scripts/library-local.sh started
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
if [[ "${SYNC:-}" == 1 ]]; then
  : "${LINGER_TEST_CONVEX_URL:?SYNC=1 needs server/scripts/library-local.sh}" "${LINGER_TEST_TOKEN:?}"
  export LINGER_API_TOKEN="$LINGER_TEST_TOKEN" LINGER_CONVEX_URL="$LINGER_TEST_CONVEX_URL"
  export LINGER_KEYCHAIN_SERVICE="${LINGER_KEYCHAIN_SERVICE:-linger-bench}"
  # The library consent sheet accepted, so the engine starts with the window.
  python3 - "$HOME_DIR/study.sqlite" <<'PY'
import json, sqlite3, sys
db = sqlite3.connect(sys.argv[1])
row = db.execute("SELECT value FROM settings WHERE key = 'app'").fetchone()
app = json.loads(row[0]) if row else {}
app["library_consent"] = True
db.execute("INSERT INTO settings(key, value) VALUES('app', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", (json.dumps(app),))
db.commit()
PY
  LINGER_HOME="$HOME_DIR" LINGER_STARTUP_PROBE=1 "$BIN" >/dev/null 2>&1 || true
fi

now_ms() { python3 -c 'import time; print(time.time()*1000)'; }
built=() presented=() external=() floor_built=() floor_presented=()
for _ in $(seq 1 "$RUNS"); do
  # Baseline: an empty eframe window with the same options (the platform's floor).
  fl=$("$FLOOR" 2>/dev/null | tail -1)
  floor_built+=("$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["first_frame_built_ms"])' "$fl")")
  floor_presented+=("$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["first_frame_presented_ms"])' "$fl")")
  start=$(now_ms)
  line=$(LINGER_HOME="$HOME_DIR" LINGER_STARTUP_PROBE=1 "$BIN" 2>/dev/null | tail -1)
  b=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["first_frame_built_ms"])' "$line")
  p=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["first_frame_presented_ms"])' "$line")
  e=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["epoch_ms"] - float(sys.argv[2]))' "$line" "$start")
  built+=("$b") presented+=("$p") external+=("$e")
done
python3 - "${built[*]}" "${presented[*]}" "${external[*]}" "${floor_built[*]}" "${floor_presented[*]}" <<'PY'
import statistics, sys
names = [
    "linger: first frame built",
    "linger: first frame presented",
    "linger: launch → first frame presented (wall)",
    "empty eframe window: first frame built",
    "empty eframe window: first frame presented",
]
for name, arg in zip(names, sys.argv[1:]):
    v = sorted(float(x) for x in arg.split())
    print(f"{name:<44} median {statistics.median(v):7.1f} ms   min {v[0]:7.1f}   max {v[-1]:7.1f}   (n={len(v)})")
PY
echo "renderer: ${LINGER_RENDERER:-default}, sync: ${SYNC:-off}"
echo "scrolling (900 frames, ~2,200 px/s through consecutive chapters):"
LINGER_HOME="$HOME_DIR" LINGER_SCROLL_PROBE=1 "$BIN" 2>/dev/null | tail -1
rm -rf "$HOME_DIR"
