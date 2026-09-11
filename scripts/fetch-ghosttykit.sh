#!/usr/bin/env bash
set -euo pipefail

SHA="${GHOSTTY_SHA:-bc9be90a21997a4e5f06bf15ae2ec0f937c2dc42}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/Frameworks/GhosttyKit.xcframework"
URL="https://github.com/manaflow-ai/ghostty/releases/download/xcframework-$SHA/GhosttyKit.xcframework.tar.gz"
CACHE="$HOME/.cache/cmux/ghosttykit/$SHA/GhosttyKit.xcframework"

expected="$(awk -v sha="$SHA" '$1 == sha { print $2; exit }' "$ROOT/scripts/ghosttykit.sha256")"
[ -n "$expected" ] || { echo "no checksum pinned for $SHA" >&2; exit 1; }

if [ -f "$OUT/.ghostty_sha" ] && [ "$(cat "$OUT/.ghostty_sha")" = "$SHA" ]; then
  echo "GhosttyKit.xcframework already at $SHA"
  exit 0
fi

mkdir -p "$ROOT/Frameworks"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if curl --fail --show-error --location --retry 3 --retry-all-errors \
     -o "$tmp/kit.tar.gz" "$URL"; then
  actual="$(shasum -a 256 "$tmp/kit.tar.gz" | awk '{print $1}')"
  [ "$actual" = "$expected" ] || {
    echo "checksum mismatch: expected $expected, got $actual" >&2; exit 1; }
  tar xzf "$tmp/kit.tar.gz" -C "$tmp"
  rm -rf "$OUT"
  mv "$tmp/GhosttyKit.xcframework" "$OUT"
elif [ -d "$CACHE" ]; then
  echo "download failed; copying from $CACHE" >&2
  rm -rf "$OUT"
  cp -R "$CACHE" "$OUT"
else
  echo "download failed and no cache at $CACHE" >&2
  exit 1
fi

echo "$SHA" > "$OUT/.ghostty_sha"
echo "GhosttyKit.xcframework ready at $SHA"
