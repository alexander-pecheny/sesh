#!/usr/bin/env bash
# Publishes the herdr fork as herdr-sesh for every Host platform as a GitHub release, and pins
# what was published in Resources/herdr.json, which the helper installer puts beside the helper.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
repo=alexander-pecheny/sesh
out="$root/build/helpers"
"$root/scripts/build-herdr.sh" >/dev/null
version=$(cat "$out/herdr-version")
tag="herdr-sesh-$version"

if ! gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
    # Forgejo push-mirrors to GitHub and prunes refs GitHub alone has, so the tag starts there.
    git -C "$root" tag -f "$tag" >/dev/null
    git -C "$root" push -q origin "refs/tags/$tag"
    curl -fsS -X POST -H "Authorization: token $(cat ~/.config/forgejo/token)" \
        https://code.pecheny.me/api/v1/repos/pecheny/sesh/push_mirrors-sync || true
    until gh api "repos/$repo/git/ref/tags/$tag" >/dev/null 2>&1; do sleep 5; done
    gh release create "$tag" -R "$repo" --verify-tag --title "$tag" \
        --notes "The herdr fork at $version, which Sesh installs on each Host as herdr-sesh to run its Tasks in. Apache-2.0; source at https://code.pecheny.me/pecheny/herdr." \
        "$out"/herdr-sesh-*.gz
fi

published="$out/published-herdr"
mkdir -p "$published"
gh release download "$tag" -R "$repo" -D "$published" --clobber -p 'herdr-sesh-*.gz'
uv run python - "$published" "$version" "https://github.com/$repo/releases/download/$tag/" \
    > "$root/Resources/herdr.json" <<'PY'
import hashlib, json, pathlib, sys

folder, version, url = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
sha256 = {
    f.name.removeprefix("herdr-sesh-").removesuffix(".gz"): hashlib.sha256(f.read_bytes()).hexdigest()
    for f in sorted(folder.glob("*.gz"))
}
print(json.dumps({"version": version, "url": url, "sha256": sha256}, indent=2))
PY
cat "$root/Resources/herdr.json"
