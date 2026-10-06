#!/usr/bin/env bash
# Builds sesh-transcript for every Host platform into build/helpers, each named after what
# `uname -sm` prints there, plus the version they all report, for the app to bundle.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
# Pure Rust needs no C toolchain: rust-lld links musl from the target's own libc.
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld

root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/build/helpers"
mkdir -p "$out"
cd "$root/core"
for pair in x86_64-unknown-linux-musl:linux-x86_64 aarch64-unknown-linux-musl:linux-aarch64 \
    aarch64-apple-darwin:darwin-arm64 x86_64-apple-darwin:darwin-x86_64; do
    target=${pair%%:*}
    rustup target add "$target" >/dev/null 2>&1
    cargo build --quiet --profile helper --target "$target" -p sesh-transcript
    cp "target/$target/helper/sesh-transcript" "$out/sesh-transcript-${pair##*:}"
done
"$out/sesh-transcript-darwin-$(uname -m)" --version > "$out/version"
ls -l "$out"
