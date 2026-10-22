#!/usr/bin/env bash
# Builds linger-core for iOS as apps/ios/LingerCore/LingerFFI.xcframework and regenerates the Swift
# bindings in apps/ios/LingerCore/Sources/LingerCore/Generated/. Run it after changing crates/core
# or crates/ffi; Xcode and `swift test` use whatever it last produced.
#
#   scripts/build-ios-core.sh              # release build
#   PROFILE=dev scripts/build-ios-core.sh  # faster unoptimized build while iterating
#
# Slices: iPhone (arm64), the simulator on Apple silicon (arm64), and macOS (arm64) so that
# `swift test` can run the package's tests without a simulator.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-ios-core.sh must run on macOS (needs Xcode's iOS SDKs and xcodebuild)" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PROFILE="${PROFILE:-release}"
OUT_PROFILE="$PROFILE"
[[ "$PROFILE" == "dev" ]] && OUT_PROFILE="debug"
PACKAGE="apps/ios/LingerCore"
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)

# Also reach the C code that cc builds (SQLite, zstd). Keep in step with apps/ios/project.yml and
# Package.swift.
export IPHONEOS_DEPLOYMENT_TARGET=26.1
export MACOSX_DEPLOYMENT_TARGET=14.0

rustup target add "${TARGETS[@]}" >/dev/null
for target in "${TARGETS[@]}"; do
  cargo build -p linger-ffi --lib --profile "$PROFILE" --target "$target"
done

# The bindings describe the library's interface, which is the same on every target.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cargo run -q -p xtask --bin uniffi-bindgen -- generate \
  --library "target/aarch64-apple-darwin/$OUT_PROFILE/liblinger_ffi.dylib" \
  --language swift --out-dir "$WORK/bindings"

GENERATED="$PACKAGE/Sources/LingerCore/Generated"
rm -rf "$GENERATED"
mkdir -p "$GENERATED" "$WORK/headers"
cp "$WORK/bindings/linger_ffi.swift" "$GENERATED/"
cp "$WORK/bindings/linger_ffiFFI.h" "$WORK/headers/"
cp "$WORK/bindings/linger_ffiFFI.modulemap" "$WORK/headers/module.modulemap"

rm -rf "$PACKAGE/LingerFFI.xcframework"
args=()
for target in "${TARGETS[@]}"; do
  args+=(-library "target/$target/$OUT_PROFILE/liblinger_ffi.a" -headers "$WORK/headers")
done
xcodebuild -create-xcframework "${args[@]}" -output "$PACKAGE/LingerFFI.xcframework" >/dev/null
echo "Built $PACKAGE/LingerFFI.xcframework ($PROFILE) and $GENERATED/linger_ffi.swift"
