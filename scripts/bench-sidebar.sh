#!/usr/bin/env bash
# Measures the sidebar's open/close transitions: how the frames around each toggle are paced,
# what each frame costs, how many chapters are typeset, and whether drawing stops afterwards.
#
#   scripts/bench-sidebar.sh                 # every scenario, 8 toggles each
#   TOGGLES=12 scripts/bench-sidebar.sh default compact
#   BIN=/path/to/linger scripts/bench-sidebar.sh
#   TRACE=1 scripts/bench-sidebar.sh compact # also every frame, to <scenario>.trace
#
# macOS doesn't draw a covered window, so there the binary is wrapped in a temporary bundle and
# opened through LaunchServices, which brings it to the front. Keep it on the display under
# test: ProMotion panels pace at 120 Hz.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/linger}"
TOGGLES="${TOGGLES:-8}"
[[ -x "$BIN" ]] || (cd "$ROOT" && cargo build --release -p linger)
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ "$(uname -s)" == "Darwin" ]]; then
  APP="$WORK/Linger.app"
  mkdir -p "$APP/Contents/MacOS"
  cp "$BIN" "$APP/Contents/MacOS/linger"
  cat >"$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Linger</string>
  <key>CFBundleIdentifier</key><string>com.wesleycole.linger.bench</string>
  <key>CFBundleExecutable</key><string>linger</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
  codesign --force --sign - "$APP" >/dev/null 2>&1 || true
fi

# probe <data dir> <demo steps> <stdout> <stderr>
probe() {
  if [[ -n "${APP:-}" ]]; then
    open -n -W --env LINGER_HOME="$1" --env "LINGER_DEMO=$2" --env LINGER_SIDEBAR_PROBE="$TOGGLES" \
      --env LINGER_SIDEBAR_PROBE_TRACE="${TRACE:-}" --stdout "$3" --stderr "$4" "$APP"
  else
    LINGER_HOME="$1" LINGER_DEMO="$2" LINGER_SIDEBAR_PROBE="$TOGGLES" \
      LINGER_SIDEBAR_PROBE_TRACE="${TRACE:-}" "$BIN" >"$3" 2>"$4"
  fi
}

# run <name> <window size> <settings JSON fields> [demo steps]
run() {
  local name="$1" size="$2" fields="$3" demo="${4:-}"
  local home="$WORK/home-$name"
  mkdir -p "$home"
  # Importing an empty notes file creates the database without a window; the settings are then
  # written into it.
  printf '{"app":"linger","format":1,"exported_at":"2026-01-01T00:00:00Z","translation":"KJV","highlights":[],"notes":[],"bookmarks":[]}' \
    >"$home/notes.json"
  LINGER_HOME="$home" "$BIN" import-notes "$home/notes.json" >/dev/null
  printf '{"translation":"kjv",%s}' "$fields" >"$home/settings.json"
  sqlite3 "$home/study.sqlite" \
    "INSERT OR REPLACE INTO settings(key, value) VALUES('app', CAST(readfile('$home/settings.json') AS TEXT));"
  probe "$home" "size:$size;$demo" "$WORK/out" "$WORK/err"
  printf '%-18s %s\n' "$name" "$(tail -1 "$WORK/out")"
  if [[ -n "${TRACE:-}" ]]; then
    grep '^toggle' "$WORK/err" >"$name.trace" || true
  fi
}

scenarios=("$@")
[[ ${#scenarios[@]} -gt 0 ]] ||
  scenarios=(default dark sepia compact medium parallel parallel-compact psalm119-compact reduce-motion)
for s in "${scenarios[@]}"; do
  case "$s" in
    default) run default 1280x800 '"theme":"Light"' ;;
    dark) run dark 1280x800 '"theme":"Dark"' ;;
    sepia) run sepia 1280x800 '"theme":"Sepia"' ;;
    compact) run compact 720x800 '"theme":"Light"' ;;
    medium) run medium 960x800 '"theme":"Light"' ;;
    parallel) run parallel 1280x800 '"theme":"Light","parallel_on":true,"parallel":["web"]' ;;
    parallel-compact) run parallel-compact 720x800 '"theme":"Light","parallel_on":true,"parallel":["web"]' ;;
    psalm119-compact) run psalm119-compact 720x800 '"theme":"Light"' 'jump:ps 119:40' ;;
    reduce-motion) run reduce-motion 720x800 '"theme":"Light","reduce_motion":true' ;;
    *) echo "unknown scenario: $s" >&2; exit 1 ;;
  esac
done
