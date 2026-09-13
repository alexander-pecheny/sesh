# Licences

What Sesh ships inside its binary, and what a Licences page has to name. Gathered for a
TestFlight release; nothing here is legal advice.

## Mosh on the App Store

Mosh's copyright holders have waived the conflict. `COPYING.iOS` in the mosh tree, drafted
with the Software Freedom Law Center, says they will not pursue a violation that results
solely from the clash between the GPL v3 and Apple's terms, as long as you comply with the
GPL in every other respect, which means giving users the source and the licence text.

That is how Blink ships: the whole app is GPL-3.0, the source is public, and it sells on
the App Store. VLC had no such waiver in 2011, which is why it was pulled.

So mosh is not a blocker. The conditions are, and Sesh meets none of them yet:

1. **Licence the app GPL-3.0-or-later.** `sesh-core` links mosh, so the combined work is
   already a derivative. There is no licence file in this repo at all. Add `COPYING` with
   the GPL v3 text and a `license` field to `sesh-core`.
2. **Publish the corresponding source** for whatever build reaches TestFlight, and keep it
   reachable for as long as the build is out there.
3. **Show users the licence and where the source lives**, from the Licences page.
4. Copy `COPYING.iOS` into `vendor/rmosh` and this repo, so the waiver travels with the
   code rather than living only in upstream's tree.

Worth noting that rmosh is a hand port by the same person who owns Sesh, so its new code
is ours to license; the parts that derive from mosh carry upstream's GPL and upstream's
waiver.

## What ships

| Component | Licence | Where |
| --- | --- | --- |
| libghostty, from the [cmux fork](https://github.com/manaflow-ai/ghostty) at `bc9be90` | MIT, Mitchell Hashimoto and Ghostty contributors | `Frameworks/GhosttyKit.xcframework` |
| [rmosh](https://code.pecheny.me/pecheny/rmosh), 8 crates | GPL-3.0-or-later, with upstream's App Store waiver | `vendor/rmosh`, see above |
| [russh](https://crates.io/crates/russh) and 220-odd other Rust crates | permissive, see below | `Frameworks/SeshCore.xcframework` |
| JetBrainsMono Nerd Font | SIL Open Font License 1.1 | `Resources/Fonts`, licence in `OFL.txt` |
| [Lucide](https://lucide.dev) icons | ISC, Lucide Icons and Contributors | `Resources/Icons.xcassets`, licence in `Resources/LICENSE-lucide.txt` |
| [Catppuccin](https://github.com/catppuccin/catppuccin) palettes | MIT | `Sesh/Catppuccin.swift`, `Resources/ghostty/themes` |
| App-icon cloud photographs | CC0 1.0, no attribution required | `Resources/icon-clouds`, provenance in `SOURCES.md` |

## The Rust crates

234 packages in the graph. Every one is permissive apart from rmosh's eight:

| Licence | Packages |
| --- | --- |
| MIT or Apache-2.0, in one spelling or another | 191 |
| MIT alone | 12 |
| Apache-2.0 alone | 8 |
| GPL-3.0-or-later | 8, all rmosh |
| BSD-3-Clause | 3 |
| Zlib, ISC, Unlicense, 0BSD, Unicode-3.0, BSD-2-Clause | the rest |

Two `r-efi` versions carry an LGPL-2.1 option, but they build only for UEFI targets, are
never linked into iOS, and offer MIT anyway.

`sesh-core` itself has no `license` field. It has to be GPL-3.0-or-later, because it
links mosh.

Regenerate the list with:

```
cd core && cargo metadata --format-version 1 --all-features \
  | python3 -c 'import json,sys; [print(p["name"], p["version"], p.get("license")) for p in json.load(sys.stdin)["packages"]]'
```

## Still to gather

- The full licence text for every crate, not just the SPDX name. MIT and BSD ask for the
  copyright line to travel with the binary, and `cargo metadata` does not carry it.
  `cargo about` or `cargo bundle-licenses` reads it from the crate sources.
- Whether the cmux fork changed anything that needs its own notice on top of upstream's.
- Apple's own requirement: an App Store listing needs a support URL and a privacy policy,
  and the privacy nutrition label has to answer for the Keychain and for hostnames typed
  into Hosts.

## The page itself

Generate it rather than hand-writing it, so it cannot drift from `Cargo.lock`. The shape
that fits this repo is a build step that writes a JSON file into `Resources`, which a
SwiftUI list reads, in the way `scripts/appicon.py` already generates an asset.
