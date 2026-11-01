#!/usr/bin/env bash
# Builds sesh-mux from mux/ for every Host platform, gzipped into build/helpers beside the
# transcript helper. The ghostty submodule must be checked out.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/build/helpers"
mkdir -p "$out"
git -C "$root" submodule update --init mux/vendor/ghostty
cd "$root/mux"

export CC_x86_64_unknown_linux_musl="$root/scripts/zig-cc"
export CC_aarch64_unknown_linux_musl="$root/scripts/zig-cc"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export CARGO_PROFILE_RELEASE_STRIP=symbols
for pair in x86_64-unknown-linux-musl:linux-x86_64 aarch64-unknown-linux-musl:linux-aarch64 \
    aarch64-apple-darwin:darwin-arm64 x86_64-apple-darwin:darwin-x86_64; do
    target=${pair%%:*}
    rustup target add "$target" >/dev/null 2>&1
    cargo build --quiet --release --target "$target"
    gzip -9 -c "target/$target/release/sesh-mux" > "$out/sesh-mux-${pair##*:}.gz"
done
