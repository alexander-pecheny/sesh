#!/usr/bin/env bash
# Builds sesh-core for device and simulator and wraps them in Frameworks/SeshCore.xcframework.
set -euo pipefail

# Homebrew's rust shadows rustup's, and only rustup carries the iOS targets.
export PATH="$HOME/.cargo/bin:$PATH"

root=$(cd "$(dirname "$0")/.." && pwd)
targets=(aarch64-apple-ios aarch64-apple-ios-sim)

cd "$root/core"
for target in "${targets[@]}"; do
    cargo build --release --target "$target" -p sesh-core
done

# The header goes next to the bridging header, not into the xcframework: GhosttyKit's
# module map claims every header in the shared include/ directory as its own.
cbindgen --config cbindgen.toml --crate sesh-core --output "$root/Sesh/sesh.h" --quiet

output="$root/Frameworks/SeshCore.xcframework"
rm -rf "$output"
xcodebuild -create-xcframework \
    -library "target/aarch64-apple-ios/release/libsesh_core.a" \
    -library "target/aarch64-apple-ios-sim/release/libsesh_core.a" \
    -output "$output" >/dev/null
echo "built $output"
