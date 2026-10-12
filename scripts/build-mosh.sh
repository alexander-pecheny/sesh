#!/usr/bin/env bash
# Builds rmosh for Sesh. `client` makes the universal mosh and mosh-client the Mac app carries,
# into build/mosh; `server` makes mosh-server for every Host platform, gzipped into build/helpers
# beside the transcript helper, with the rmosh commit they were built from in mosh-version.
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root/vendor/rmosh"

case ${1:-} in
client)
    out="$root/build/mosh"
    mkdir -p "$out"
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        rustup target add "$target" >/dev/null 2>&1
        cargo build --quiet --release --target "$target" -p mosh -p mosh-client
    done
    for bin in mosh mosh-client; do
        lipo -create -output "$out/$bin" target/{aarch64,x86_64}-apple-darwin/release/$bin
    done
    ;;
server)
    out="$root/build/helpers"
    mkdir -p "$out"
    # mosh-server never reads terminfo, but rmosh links curses for the client, and musl has
    # none; stubs that report no database satisfy the linker.
    stubs="$root/build/mosh/nocurses"
    mkdir -p "$stubs"
    printf 'int setupterm(const char *t, int fd, int *e) { if (e) *e = -1; return -1; }\n%s\n%s\n' \
        'char *tigetstr(const char *n) { return (char *)-1; }' 'int tigetflag(const char *n) { return -1; }' > "$stubs/stub.c"
    for arch in x86_64 aarch64; do
        mkdir -p "$stubs/$arch"
        zig cc -target "$arch-linux-musl" -O2 -c "$stubs/stub.c" -o "$stubs/$arch/stub.o"
        zig ar rcs "$stubs/$arch/libcurses.a" "$stubs/$arch/stub.o"
    done
    export CC_x86_64_unknown_linux_musl="$root/scripts/zig-cc"
    export CC_aarch64_unknown_linux_musl="$root/scripts/zig-cc"
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-L $stubs/x86_64"
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-L $stubs/aarch64"
    for pair in x86_64-unknown-linux-musl:linux-x86_64 aarch64-unknown-linux-musl:linux-aarch64 \
        aarch64-apple-darwin:darwin-arm64 x86_64-apple-darwin:darwin-x86_64; do
        target=${pair%%:*}
        rustup target add "$target" >/dev/null 2>&1
        cargo build --quiet --release --target "$target" -p mosh-server
        gzip -9 -c "target/$target/release/mosh-server" > "$out/mosh-server-${pair##*:}.gz"
    done
    git rev-parse --short=12 HEAD > "$out/mosh-version"
    ;;
*)
    echo "usage: $0 client|server" >&2
    exit 2
    ;;
esac
