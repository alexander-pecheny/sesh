#!/usr/bin/env python3
"""Writes Resources/licenses.json: every component the app links or bundles, with its
licence text. Run it whenever Cargo.lock changes; the Licences screen reads the JSON."""
import json, os, re, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO = os.path.expanduser("~/.cargo/bin/cargo")
TARGET = "aarch64-apple-ios"
# russh and its two helper crates declare Apache-2.0 and ship no licence file.
FALLBACK = ROOT / "Resources/LICENSE-Apache-2.0.txt"

BUNDLED = [
    ("libghostty", "cmux fork bc9be90", "MIT", ["Resources/LICENSE-ghostty.txt"]),
    ("JetBrainsMono Nerd Font", "2.304", "OFL-1.1", ["Resources/Fonts/OFL.txt"]),
    ("Lucide icons", "", "ISC", ["Resources/LICENSE-lucide.txt"]),
    ("Catppuccin palettes", "", "MIT", ["Resources/LICENSE-catppuccin.txt"]),
]


def linked_crates():
    out = subprocess.run(
        [CARGO, "tree", "-e", "normal", "--target", TARGET, "-p", "sesh-core",
         "--prefix", "none", "--no-dedupe"],
        cwd=ROOT / "core", capture_output=True, text=True, check=True).stdout
    crates = {}
    for line in out.splitlines():
        m = re.match(r"^([a-zA-Z0-9_.-]+) v([0-9][^ ]*)(?: \((.*)\))?$", line.strip())
        if m:
            crates[(m.group(1), m.group(2))] = m.group(3)
    return crates


def spdx():
    meta = json.loads(subprocess.run(
        [CARGO, "metadata", "--format-version", "1", "--all-features"],
        cwd=ROOT / "core", capture_output=True, text=True, check=True).stdout)
    return {(p["name"], p["version"]): p.get("license") or "" for p in meta["packages"]}


def read_texts(directory):
    if not directory or not Path(directory).is_dir():
        return ""
    files = sorted(p for p in Path(directory).iterdir()
                   if p.is_file() and re.match(r"(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)",
                                               p.name, re.I))
    parts = []
    for f in files[:4]:
        try:
            parts.append(f.read_text(errors="replace").strip())
        except OSError:
            pass
    return parts


def registry_dir(name, version):
    for src in (Path.home() / ".cargo/registry/src").glob("*"):
        d = src / f"{name}-{version}"
        if d.is_dir():
            return d
    return None


def main():
    licenses = spdx()
    entries = []
    for (name, version), path in sorted(linked_crates().items()):
        if name == "sesh-core":
            continue
        directory = path if path else registry_dir(name, version)
        text = read_texts(directory)
        if not text and path:  # rmosh crates share one COPYING at the repo root
            text = read_texts(ROOT / "vendor/rmosh")
        license_id = licenses.get((name, version), "")
        if not text and license_id == "Apache-2.0":
            text = [FALLBACK.read_text().strip()]
        entries.append({"name": name, "version": version,
                        "license": license_id, "text": text})
    for name, version, spdx_id, paths in BUNDLED:
        text = [(ROOT / p).read_text(errors="replace").strip()
                for p in paths if (ROOT / p).is_file()]
        entries.insert(0, {"name": name, "version": version, "license": spdx_id,
                           "text": text})
    missing = [e["name"] for e in entries if not e["text"]]
    # Half the crates ship the same Apache text beside their own MIT one, so the pool
    # holds each distinct file once and a component names the ones it carries.
    pool = {}
    for e in entries:
        e["texts"] = [pool.setdefault(t, len(pool)) for t in e.pop("text")]
    out = ROOT / "Resources/licenses.json"
    gpl = pool.setdefault((ROOT / "COPYING").read_text().strip(), len(pool))
    waiver = pool.setdefault((ROOT / "COPYING.iOS").read_text().strip(), len(pool))
    out.write_text(json.dumps({"texts": list(pool), "components": entries,
                               "gpl": gpl, "waiver": waiver}))
    print(f"{len(entries)} components -> {out.relative_to(ROOT)}")
    if missing:
        print("no licence text found for:", ", ".join(missing), file=sys.stderr)
        sys.exit(1)


main()
