#!/usr/bin/env bash
# Builds GhosttyKit.xcframework from the pinned cmux-fork commit. Needs Zig 0.16.
set -euo pipefail

SHA="${GHOSTTY_SHA:-ef4f1570d62117ebc51e3b0f07ad4f34032267b6}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/Frameworks/GhosttyKit.xcframework"
WORK="${GHOSTTYKIT_WORK:-$ROOT/.local/ghostty}"

if [ -f "$OUT/.sesh_sha" ] && [ "$(cat "$OUT/.sesh_sha")" = "$SHA" ]; then
  echo "GhosttyKit.xcframework already built for $SHA"
  exit 0
fi

mkdir -p "$ROOT/Frameworks"

if [ ! -d "$WORK/.git" ]; then
  git clone --quiet https://github.com/manaflow-ai/ghostty.git "$WORK"
fi
git -C "$WORK" fetch --quiet origin "$SHA" 2>/dev/null || true
git -C "$WORK" checkout --quiet --force "$SHA"
git -C "$WORK" clean -qfd -e zig-out -e .zig-cache

(cd "$WORK" && zig build -Dxcframework-target=universal -Demit-xcframework=true \
  -Demit-macos-app=false -Doptimize=ReleaseFast)

rm -rf "$OUT"
cp -R "$WORK/macos/GhosttyKit.xcframework" "$OUT"
echo "$SHA" > "$OUT/.sesh_sha"
echo "GhosttyKit.xcframework built for $SHA"
