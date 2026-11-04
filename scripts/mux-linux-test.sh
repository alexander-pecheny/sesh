#!/usr/bin/env bash
# Runs sesh-mux's tests on Linux from a Mac, in a container: its CLI, attach and handoff tests
# build only there. Arguments go to cargo test, e.g. `just mux-linux-test --test cli agents`.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
docker info >/dev/null 2>&1 || colima start >/dev/null
git -C "$root" submodule update --init mux/vendor/ghostty
docker build -q -t sesh-mux-linux-test "$root/scripts/linux-test" >/dev/null
docker run --rm -v "$root/mux":/src:ro -v sesh-mux-linux-target:/target -w /src sesh-mux-linux-test cargo test "$@"
