#!/usr/bin/env bash
# Publishes sesh-transcript for every Host platform as a GitHub release, and pins what was
# published in Resources/helpers.json, the only part of the helpers the app carries.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
repo=alexander-pecheny/sesh
out="$root/build/helpers"
"$root/scripts/build-helpers.sh" >/dev/null
version=$(cat "$out/version")
tag="helper-$(echo "${version#sesh-transcript }" | tr + -)"

if ! gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
    # Forgejo push-mirrors to GitHub and prunes refs GitHub alone has, so the tag starts there.
    git -C "$root" tag -f "$tag" >/dev/null
    git -C "$root" push -q origin "refs/tags/$tag"
    curl -fsS -X POST -H "Authorization: token $(cat ~/.config/forgejo/token)" \
        https://code.pecheny.me/api/v1/repos/pecheny/sesh/push_mirrors-sync || true
    until gh api "repos/$repo/git/ref/tags/$tag" >/dev/null 2>&1; do sleep 5; done
    gh release create "$tag" -R "$repo" --verify-tag --title "$tag" \
        --notes "sesh-transcript, the helper Sesh installs on each Host (docs/adr/0006)." "$out"/*.gz
fi

published="$out/published"
mkdir -p "$published"
gh release download "$tag" -R "$repo" -D "$published" --clobber -p '*.gz'
uv run python - "$published" "$version" "https://github.com/$repo/releases/download/$tag/" \
    > "$root/Resources/helpers.json" <<'EOF'
import hashlib, json, pathlib, sys

folder, version, url = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
sha256 = {
    f.name.removeprefix("sesh-transcript-").removesuffix(".gz"): hashlib.sha256(f.read_bytes()).hexdigest()
    for f in sorted(folder.glob("*.gz"))
}
print(json.dumps({"version": version, "url": url, "sha256": sha256}, indent=2))
EOF
cat "$root/Resources/helpers.json"
