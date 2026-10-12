#!/usr/bin/env bash
# Publishes rmosh's mosh-server for every Host platform as a GitHub release, and pins what was
# published in Resources/mosh-server.json, which the helper installer puts beside the helper.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
repo=alexander-pecheny/sesh
out="$root/build/helpers"
"$root/scripts/build-mosh.sh" server >/dev/null 2>&1
version=$(cat "$out/mosh-version")
tag="mosh-server-$version"

if ! gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
    # Forgejo push-mirrors to GitHub and prunes refs GitHub alone has, so the tag starts there.
    git -C "$root" tag -f "$tag" >/dev/null
    git -C "$root" push -q origin "refs/tags/$tag"
    curl -fsS -X POST -H "Authorization: token $(cat ~/.config/forgejo/token)" \
        https://code.pecheny.me/api/v1/repos/pecheny/sesh/push_mirrors-sync || true
    until gh api "repos/$repo/git/ref/tags/$tag" >/dev/null 2>&1; do sleep 5; done
    gh release create "$tag" -R "$repo" --verify-tag --title "$tag" \
        --notes "rmosh's mosh-server at $version, which Sesh installs on each Host beside its helper. GPL-3.0-or-later; source at https://code.pecheny.me/pecheny/rmosh." \
        "$out"/mosh-server-*.gz
fi

published="$out/published-mosh"
mkdir -p "$published"
gh release download "$tag" -R "$repo" -D "$published" --clobber -p 'mosh-server-*.gz'
uv run python - "$published" "$version" "https://github.com/$repo/releases/download/$tag/" \
    > "$root/Resources/mosh-server.json" <<'PY'
import hashlib, json, pathlib, sys

folder, version, url = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
sha256 = {
    f.name.removeprefix("mosh-server-").removesuffix(".gz"): hashlib.sha256(f.read_bytes()).hexdigest()
    for f in sorted(folder.glob("*.gz"))
}
print(json.dumps({"version": version, "url": url, "sha256": sha256}, indent=2))
PY
cat "$root/Resources/mosh-server.json"
