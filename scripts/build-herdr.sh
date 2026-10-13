#!/usr/bin/env bash
# Builds the herdr fork as herdr-sesh for every Host platform, gzipped into build/helpers beside
# the transcript helper, with the fork commit they were built from in herdr-version. The fork is
# the checkout HERDR_SRC names, ~/herdr by default, and must be clean and pushed.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
root=$(cd "$(dirname "$0")/.." && pwd)
src=${HERDR_SRC:-$HOME/herdr}
out="$root/build/helpers"
mkdir -p "$out"
cd "$src"
if [ -n "$(git status --porcelain)" ] || [ -z "$(git branch -r --contains HEAD)" ]; then
    echo "$src has changes that are not committed and pushed" >&2
    exit 1
fi

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
    gzip -9 -c "target/$target/release/herdr" > "$out/herdr-sesh-${pair##*:}.gz"
done
git rev-parse --short=12 HEAD > "$out/herdr-version"
