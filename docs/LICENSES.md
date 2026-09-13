# Licences

What Sesh ships inside its binary, and what a Licences page has to name. Gathered for a
TestFlight release; nothing here is legal advice.

## The mosh problem

Sesh cannot go on TestFlight or the App Store as it stands.

`sesh-core` links `mosh`, `mosh-client` and `mosh-sys` from `vendor/rmosh`, and rmosh is
**GPL-3.0-or-later**. Its `COPYING` is the GPL v3, its `Cargo.toml` says
`license = "GPL-3.0-or-later"`, and its `AUTHORS` lists Keith Winstein and the other
upstream mosh authors. The README calls rmosh a port of mosh written module by module
against the C++, so it is a derivative work and cannot be relicensed by us alone.

Apple's distribution terms restrict what a recipient may do with the binary, which is the
conflict the GPL's section 6 and section 12 describe. VLC was pulled from the App Store
over the same clash in 2011 and had to move to the LGPL. TestFlight is the same act of
conveying a binary through the same terms, so it carries the same problem.

Three ways out, in the order I would try them:

1. Ship an App Store build with ssh only, and keep mosh for builds installed by hand.
   `sesh-core` would need mosh behind a cargo feature so the GPL crates are not linked at
   all. This is the only option that is entirely in our hands.
2. Ask mosh's copyright holders to relicense. Mosh has many contributors and all of them
   would have to agree, so this is slow and probably fails.
3. Distribute outside the App Store, through an alternative marketplace in the EU or a
   sideloading route. That gives up TestFlight.

Everything below assumes this gets resolved.

## What ships

| Component | Licence | Where |
| --- | --- | --- |
| libghostty, from the [cmux fork](https://github.com/manaflow-ai/ghostty) at `bc9be90` | MIT, Mitchell Hashimoto and Ghostty contributors | `Frameworks/GhosttyKit.xcframework` |
| [rmosh](https://code.pecheny.me/pecheny/rmosh), 8 crates | **GPL-3.0-or-later** | `vendor/rmosh`, see above |
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

`sesh-core` itself has no `license` field. Set one before release.

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
