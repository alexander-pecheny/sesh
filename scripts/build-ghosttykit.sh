#!/usr/bin/env bash
# Builds GhosttyKit.xcframework from the pinned cmux-fork commit with
# patches/libxev-ios-async.patch applied. The published prebuilt cannot be used on
# iOS: libxev disables its cross-thread wakeup off macOS, so libghostty's renderer
# and IO threads never wake and the terminal neither draws nor writes.
set -euo pipefail

SHA="${GHOSTTY_SHA:-bc9be90a21997a4e5f06bf15ae2ec0f937c2dc42}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/Frameworks/GhosttyKit.xcframework"
WORK="${GHOSTTYKIT_WORK:-$ROOT/.local/ghostty}"
LIBXEV="libxev-0.0.0-86vtc4IcEwCqEYxEYoN_3KXmc6A9VLcm22aVImfvecYs"

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
git -C "$WORK" clean -qfd -e vendor-patched -e zig-out -e .zig-cache

rm -rf "$WORK/vendor-patched"
mkdir -p "$WORK/vendor-patched"
if [ ! -d "$HOME/.cache/zig/p/$LIBXEV" ]; then
  zig fetch "https://deps.files.ghostty.org/libxev-34fa50878aec6e5fa8f532867001ab3c36fae23e.tar.gz" >/dev/null
fi
cp -R "$HOME/.cache/zig/p/$LIBXEV" "$WORK/vendor-patched/libxev"
chmod -R u+w "$WORK/vendor-patched/libxev"
patch -s -p1 -d "$WORK/vendor-patched/libxev" < "$ROOT/patches/libxev-ios-async.patch"

perl -0777 -i -pe 's{\.libxev = \.\{.*?\n        \},}{.libxev = .{\n            .path = "vendor-patched/libxev",\n            .lazy = true,\n        },}s' "$WORK/build.zig.zon"

(cd "$WORK" && zig build -Dxcframework-target=universal -Demit-xcframework=true \
  -Demit-macos-app=false -Doptimize=ReleaseFast)

rm -rf "$OUT"
cp -R "$WORK/macos/GhosttyKit.xcframework" "$OUT"
echo "$SHA" > "$OUT/.sesh_sha"
echo "GhosttyKit.xcframework built for $SHA"
