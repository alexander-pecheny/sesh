#!/usr/bin/env bash
# Publishes sesh-mux for every Host platform as a GitHub release, and pins what was published in
# Resources/sesh-mux.json, which the helper installer puts beside the helper.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
repo=alexander-pecheny/sesh
out="$root/build/helpers"
"$root/scripts/build-mux.sh" >/dev/null
version=$(cat "$out/mux-version")
tag="sesh-mux-$version"

if ! gh release view "$tag" -R "$repo" >/dev/null 2>&1; then
    # Forgejo push-mirrors to GitHub and prunes refs GitHub alone has, so the tag starts there.
    git -C "$root" tag -f "$tag" >/dev/null
    git -C "$root" push -q origin "refs/tags/$tag"
    curl -fsS -X POST -H "Authorization: token $(cat ~/.config/forgejo/token)" \
        https://code.pecheny.me/api/v1/repos/pecheny/sesh/push_mirrors-sync || true
    until gh api "repos/$repo/git/ref/tags/$tag" >/dev/null 2>&1; do sleep 5; done
    gh release create "$tag" -R "$repo" --verify-tag --title "$tag" \
        --notes "sesh-mux at $version, the terminal server Sesh installs on each Host to run its Tasks in. Apache-2.0, derived from herdr; source under mux/ in this repository." \
        "$out"/sesh-mux-*.gz
fi

published="$out/published-mux"
mkdir -p "$published"
gh release download "$tag" -R "$repo" -D "$published" --clobber -p 'sesh-mux-*.gz'
# The stamp of the published build, not of the one just made, which may be a rebuild.
gunzip -c "$published/sesh-mux-darwin-$(uname -m).gz" > "$published/sesh-mux"
chmod 755 "$published/sesh-mux"
build=$("$published/sesh-mux" --version | sed -n 's/.* build //p')
uv run python - "$published" "$version" "https://github.com/$repo/releases/download/$tag/" "${build:-0}" \
    > "$root/Resources/sesh-mux.json" <<'PY'
import hashlib, json, pathlib, sys

folder, version, url, build = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4])
sha256 = {
    f.name.removeprefix("sesh-mux-").removesuffix(".gz"): hashlib.sha256(f.read_bytes()).hexdigest()
    for f in sorted(folder.glob("*.gz"))
}
print(json.dumps({"version": version, "build": build, "url": url, "sha256": sha256}, indent=2))
PY
cat "$root/Resources/sesh-mux.json"
