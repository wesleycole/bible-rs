#!/usr/bin/env bash
# Runs `convex dev` against an anonymous local deployment. Convex's prebuilt local backend needs a
# newer glibc than some hosts (Debian 12 has 2.36); there it runs under Nix's glibc instead.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
export CONVEX_AGENT_MODE=anonymous

host_glibc() { ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$'; }

wrap_backends() {
    local bin real need
    for bin in "$HOME"/.cache/convex/binaries/*/convex-local-backend; do
        [[ -f "$bin" ]] || continue
        file -b "$bin" | grep -q ELF || continue # already wrapped
        need="$(objdump -T "$bin" | grep -oE 'GLIBC_[0-9.]+' | sed 's/GLIBC_//' | sort -V | tail -1)"
        if [[ "$(printf '%s\n%s\n' "$need" "$(host_glibc)" | sort -V | tail -1)" == "$(host_glibc)" ]]; then
            continue
        fi
        command -v nix >/dev/null || { echo "convex-local-backend needs glibc $need; install Nix or a newer distro" >&2; exit 1; }
        local glibc gcclib
        glibc="$(nix build --no-link --print-out-paths 'nixpkgs#glibc^out')"
        gcclib="$(nix build --no-link --print-out-paths 'nixpkgs#gcc.cc^lib')"
        real="$bin.real"
        mv "$bin" "$real"
        printf '#!/bin/sh\nexec %s/lib/ld-linux-x86-64.so.2 --library-path %s/lib:%s/lib:/lib/x86_64-linux-gnu %s "$@"\n' \
            "$glibc" "$glibc" "$gcclib" "$real" >"$bin"
        chmod +x "$bin"
        echo "wrapped $bin with $glibc"
    done
}

if ! compgen -G "$HOME/.cache/convex/binaries/*/convex-local-backend" >/dev/null; then
    # The first run downloads the backend (and fails to start it where glibc is too old).
    timeout 300 npx convex dev --once --typecheck disable || true
fi
wrap_backends
exec npx convex dev "$@"
