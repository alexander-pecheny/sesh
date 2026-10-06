#!/usr/bin/env bash
# Builds sesh-transcript for every Host platform into build/helpers, each named after what
# `uname -sm` prints there, plus the version they all report, for the app to bundle.
# Gzipped, because App Store validation rejects macOS executables inside an iOS app.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
root=$(cd "$(dirname "$0")/.." && pwd)
# zig compiles SQLite for musl and rust-lld links it with the target's own libc.
export CC_x86_64_unknown_linux_musl="$root/scripts/zig-cc"
export CC_aarch64_unknown_linux_musl="$root/scripts/zig-cc"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
# The Vault needs only SQLite's FTS5 and JSON, and leaving out the rest saves 80 KB.
export LIBSQLITE3_FLAGS="-USQLITE_ENABLE_FTS3 -USQLITE_ENABLE_RTREE -USQLITE_ENABLE_DBSTAT_VTAB -USQLITE_ENABLE_STAT4"

out="$root/build/helpers"
mkdir -p "$out"
cd "$root/core"
for pair in x86_64-unknown-linux-musl:linux-x86_64 aarch64-unknown-linux-musl:linux-aarch64 \
    aarch64-apple-darwin:darwin-arm64 x86_64-apple-darwin:darwin-x86_64; do
    target=${pair%%:*}
    rustup target add "$target" >/dev/null 2>&1
    cargo build --quiet --profile helper --target "$target" -p sesh-transcript
    gzip -9 -c "target/$target/helper/sesh-transcript" > "$out/sesh-transcript-${pair##*:}.gz"
done
"target/$(uname -m | sed s/arm64/aarch64/)-apple-darwin/helper/sesh-transcript" --version > "$out/version"
ls -l "$out"
