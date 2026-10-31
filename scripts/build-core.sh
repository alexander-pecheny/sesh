#!/usr/bin/env bash
# Builds sesh-core for the phone, its simulator and the Mac, and wraps them in
# Frameworks/SeshCore.xcframework.
set -euo pipefail

# Homebrew's rust shadows rustup's, and only rustup carries the iOS targets.
export PATH="$HOME/.cargo/bin:$PATH"

root=$(cd "$(dirname "$0")/.." && pwd)
targets=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin x86_64-apple-darwin)

# The apps' own deployment targets, or the linker warns of objects built for a newer OS.
export MACOSX_DEPLOYMENT_TARGET=15.0 IPHONEOS_DEPLOYMENT_TARGET=18.0

cd "$root/core"
for target in "${targets[@]}"; do
    cargo build --release --target "$target" -p sesh-core
done

# The header goes next to the bridging header, not into the xcframework: GhosttyKit's
# module map claims every header in the shared include/ directory as its own.
cbindgen --config cbindgen.toml --crate sesh-core --output "$root/Sesh/sesh.h" --quiet

# One library for both Mac architectures, as GhosttyKit ships.
mkdir -p target/macos/release
lipo -create target/aarch64-apple-darwin/release/libsesh_core.a target/x86_64-apple-darwin/release/libsesh_core.a \
    -output target/macos/release/libsesh_core.a

output="$root/Frameworks/SeshCore.xcframework"
rm -rf "$output"
xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/release/libsesh_core.a" \
    -library "target/aarch64-apple-ios-sim/release/libsesh_core.a" \
    -library "target/macos/release/libsesh_core.a" \
    -output "$output" >/dev/null
echo "built $output"
