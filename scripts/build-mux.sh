#!/usr/bin/env bash
# Builds sesh-mux from mux/ for every Host platform, gzipped into build/helpers beside the
# transcript helper, with the hash of the mux/ tree they were built from in mux-version and the
# build's UTC stamp, which orders it among others (ADR 0022), in mux-build.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/build/helpers"
mkdir -p "$out"
git -C "$root" submodule update --init mux/vendor/ghostty
if [ -n "$(git -C "$root" status --porcelain mux)" ]; then
    echo "mux/ has changes that are not committed" >&2
    exit 1
fi
cd "$root/mux"

export CC_x86_64_unknown_linux_musl="$root/scripts/zig-cc"
export CC_aarch64_unknown_linux_musl="$root/scripts/zig-cc"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export CARGO_PROFILE_RELEASE_STRIP=symbols
export SESH_BUILD=$(date -u +%Y%m%d%H%M%S)
for pair in x86_64-unknown-linux-musl:linux-x86_64 aarch64-unknown-linux-musl:linux-aarch64 \
    aarch64-apple-darwin:darwin-arm64 x86_64-apple-darwin:darwin-x86_64; do
    target=${pair%%:*}
    rustup target add "$target" >/dev/null 2>&1
    cargo build --quiet --release --target "$target"
    gzip -9 -c "target/$target/release/sesh-mux" > "$out/sesh-mux-${pair##*:}.gz"
done
git -C "$root" rev-parse --short=12 HEAD:mux > "$out/mux-version"
echo "$SESH_BUILD" > "$out/mux-build"
