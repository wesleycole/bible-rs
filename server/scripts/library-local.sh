#!/usr/bin/env bash
# Starts a throwaway local Convex backend with this checkout's functions, seeded, that accepts dev
# tokens (scripts/dev-token.ts), for core's tests against a real Convex. Leaves it running and prints
# the environment those tests read:
#
#   eval "$(server/scripts/library-local.sh)"     # LINGER_TEST_CONVEX_URL, _TOKEN, _ENV, _PID
#   kill "$LINGER_TEST_CONVEX_PID"
#
# It never touches .env.local: the CLI reaches the backend through a self-hosted env file of its own.
# On Linux, Convex's prebuilt backend needs glibc 2.39 or newer (Ubuntu 24.04 has it; Debian 12
# doesn't). The deploy bundles "use node" actions with the `node` on PATH, which must be a version
# Convex supports (20, 22 or 24).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

TAG=precompiled-2026-10-10-72ce13c
PORT=${LIBRARY_LOCAL_PORT:-3290}
DIR=$(mktemp -d "${TMPDIR:-/tmp}/linger-convex-XXXXXX")

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) TARGET=aarch64-apple-darwin ;;
    Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
    Linux-aarch64) TARGET=aarch64-unknown-linux-gnu ;;
    Linux-x86_64) TARGET=x86_64-unknown-linux-gnu ;;
    *) echo "no Convex backend build for $(uname -sm)" >&2; exit 1 ;;
esac
BIN="$HOME/.cache/convex/binaries/$TAG/convex-local-backend"
if [[ ! -x "$BIN" ]]; then
    mkdir -p "$(dirname "$BIN")"
    curl -fsSL -o "$DIR/backend.zip" \
        "https://github.com/get-convex/convex-backend/releases/download/$TAG/convex-local-backend-$TARGET.zip"
    unzip -q -o "$DIR/backend.zip" -d "$(dirname "$BIN")"
    chmod +x "$BIN"
fi
# The binary itself is the test (a host may run it through a newer libc of its own); when it can't
# start, say why it most likely can't.
if ! "$BIN" --help >/dev/null 2>&1; then
    if [[ "$(uname -s)" == Linux ]]; then
        have=$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || echo unknown)
        echo "Convex's local backend needs glibc 2.39 or newer; this host has ${have}." >&2
    else
        echo "Convex's local backend ($BIN) doesn't start on this host." >&2
    fi
    exit 1
fi

NAME=linger-test
SECRET=$(openssl rand -hex 32)
(cd "$DIR" && exec "$BIN" --instance-name "$NAME" --instance-secret "$SECRET" --interface 127.0.0.1 \
    --port "$PORT" --site-proxy-port $((PORT + 1)) --disable-beacon db.sqlite3 >backend.log 2>&1) &
PID=$!
URL="http://127.0.0.1:$PORT"
for _ in $(seq 100); do
    curl -fs "$URL/version" >/dev/null 2>&1 && break
    kill -0 "$PID" 2>/dev/null || { cat "$DIR/backend.log" >&2; exit 1; }
    sleep 0.2
done

ENV="$DIR/convex.env"
printf 'CONVEX_SELF_HOSTED_URL=%s\nCONVEX_SELF_HOSTED_ADMIN_KEY=%s\n' \
    "$URL" "$("$BIN" keygen admin-key --instance-name "$NAME" --instance-secret "$SECRET")" >"$ENV"
# The dev token key, inline: the backend can't fetch a JWKS for the dev issuer.
npx tsx scripts/dev-token.ts --env | grep -E '^CLERK_(JWT_ISSUER|JWKS)=' | while IFS='=' read -r k v; do
    npx convex env set --env-file "$ENV" "$k" "$v" >&2
done
npx convex deploy --env-file "$ENV" --typecheck disable --yes >&2
npx convex run --env-file "$ENV" admin:seed '{}' >&2

run=$(date +%s)
echo "export LINGER_TEST_CONVEX_URL=$URL"
echo "export LINGER_TEST_CONVEX_PID=$PID"
echo "export LINGER_TEST_CONVEX_ENV=$ENV"
echo "export LINGER_TEST_TOKEN=$(npx tsx scripts/dev-token.ts "user_local_$run" --oauth --minutes 120)"
