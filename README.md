# Sesh

Sesh is a native iOS terminal that reaches remote machines over SSH or mosh. Nothing runs
locally: every terminal is a window onto a remote shell. See `CONTEXT.md` for the
vocabulary, `docs/PLAN.md` for the plan and `docs/adr/` for the decisions.

## How it works

The terminal is [libghostty](https://ghostty.org) — the real grid, renderer and
escape-sequence handling — built from the cmux fork and driven in manual I/O mode, so
bytes arrive from a transport instead of a child process, which iOS would not let us
fork. A `UIView` we own holds the surface, feeds it keystrokes and touches, and pushes
remote output in. Both transports live in one Rust crate, `core/sesh-core`: russh for
SSH, [rmosh](https://code.pecheny.me/pecheny/rmosh)'s client for mosh, bootstrapped by
running `mosh-server` over a russh channel and then speaking UDP in process. Swift sees
one C ABI. Keys and saved passwords live in the Keychain; Hosts, Drafts and known hosts
are JSON in Application Support. Several Tabs may be open at once; a Tab that is not
visible keeps its Session running and only stops drawing.

Fingers are read in one of three Touch modes, picked from the selector at the left of the
keys row: Type taps as a mouse with the on-screen keyboard up, Click does the same with
the keyboard down so a TUI can be worked by touch, and Select drags out a selection to
copy. The keys row stays on screen in all three, and a hardware keyboard works in all
three.

The keys come in two rows, each scrolling on its own when it runs past the screen. The
first holds the mode selector, paste, upload, the Editor, Enter, Shift-Enter, right-click
and the arrows; the second holds Home to End, the modifiers with esc and tab, and the
symbols. Shift-Enter sends Enter with Shift rather than a bare newline, so a remote
program that reads the Kitty keyboard protocol can tell the two apart.

The upload button sends images and videos from the phone's library to the Host over SFTP
and drops their remote paths where you were typing, so a program on the far end can be
pointed at them. There is one in the keys row and one in the Draft toolbar, because the
Editor covers the keys row. Files land in `~/.sesh/uploads` under a name carrying the date
and time, and nothing ever deletes them. Images are cut to 1568px on the longest edge and
re-encoded as JPEG unless Settings says otherwise; HEIC becomes JPEG either way, since
little on a Linux box reads it. Videos go up untouched: reading them is the Host's
business. A mosh Host dials a second SSH connection for each Upload, because its own ends
once mosh-server is running.

## Prerequisites

Xcode 26.3 with an iOS 26 simulator, `just`, `xcodegen`, `zig` 0.15.2, `protoc`, `uv`,
Rust with the `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets, and `cbindgen`
(`cargo install cbindgen`). `mosh-server` from Homebrew is needed only by the local test
server. Signing uses an Apple Development identity; set `DEVELOPMENT_TEAM` in
`project.yml` to your own team.

Clone with `git submodule update --init vendor/rmosh`: mosh comes from rmosh, vendored
there, and `sesh-core` depends on its `mosh` and `mosh-client` libraries by path and never
patches them.

## Targets

- `just ghosttykit` builds the pinned `GhosttyKit.xcframework` from the cmux fork with
  `patches/libxev-ios-async.patch`, which restores libxev's cross-thread wakeup on iOS;
  without it libghostty's renderer and I/O threads never wake.
  `just ghosttykit-prebuilt` fetches the published build and verifies it against
  `scripts/ghosttykit.sha256`, but that one hangs on iOS.
- `just core` builds the Rust crate for device and simulator and wraps it as
  `Frameworks/SeshCore.xcframework`; `cbindgen` writes `Sesh/sesh.h`, which Swift reaches
  through `Sesh/Sesh-Bridging-Header.h` rather than a module map, because GhosttyKit's
  module map claims every header in the shared `include/` directory. `just core-test` runs
  the crate's tests. Both xcframeworks are gitignored.
- `just gen` regenerates the Xcode project from `project.yml`, the only place project
  settings are edited. `just build` builds for the simulator, `just run` installs and
  launches it there.
- `just sshd` starts an unprivileged sshd on 127.0.0.1:2222 with a throwaway host key and
  test key under `.local/`, and `just sshd-stop` stops it; that is what the simulator
  connects to. `just e2e` runs `scripts/e2e.py` against it and the booted simulator,
  printing a pass or fail line per check.
- `just icons` renders every app-icon variant to `build/icons` with a contact sheet, and
  `just icon NAME` installs one into `Resources/Icons.xcassets/AppIcon.appiconset`. The
  icon is generated, not drawn: `scripts/appicon.py` maps a cloud photograph's luminance
  onto a Catppuccin Mocha ramp and screens a neon "sesh" over it. A variant name joins a
  cloud, a ramp and a glow recipe, all listed in that script.
- `just site` publishes `docs/site` to <https://sesh.pecheny.me>, three static pages that
  the App Store listing needs: the app, the privacy policy and where the source lives.
  Caddy on `vps2day-ee` serves `/home/ap/sesh_site`; the DNS record is an unproxied A
  record in Cloudflare.
- `just testflight` archives, exports and uploads a build to TestFlight. It reads
  `DEVELOPMENT_TEAM`, `ASC_KEY_ID` and `ASC_ISSUER_ID` from an untracked `.env`; no Apple
  ID is signed into Xcode, so `xcodebuild` authenticates with the same App Store Connect
  API key the upload uses and mints the distribution certificate itself. The build number
  is the commit count.

Bundled JetBrainsMono Nerd Font is under the SIL Open Font License, copied into
`Resources/Fonts` with its licence. Icons are [Lucide](https://lucide.dev), ISC licence,
in `Resources/LICENSE-lucide.txt`. The app-icon cloud photographs in
`Resources/icon-clouds` are CC0; `SOURCES.md` there names them.

## Licence

Sesh's own source is MIT, in `LICENSE`. A built app is GPL-3.0-or-later, in `COPYING`,
because `sesh-core` links mosh through rmosh and the binary is then a derivative of it.
Anyone handed that binary is owed its corresponding source and the licence text.

Mosh's copyright holders waived the clash between the GPL and Apple's terms in
`COPYING.iOS`, so the App Store is open to a build that meets the GPL in every other
respect. `docs/LICENSES.md` lists what the binary carries and what a release still needs.

## Extra flags

A Host's Extra flags are parsed against a subset and anything else is refused before
connecting, naming the flag.

- ssh: `-p`, `-l`, `-4`, `-6`, `-J user@host[:port]` (one hop), `-t`, `-A`, and `-o` with
  `ServerAliveInterval`, `ServerAliveCountMax`, `Port`, `User`, `HostKeyAlgorithms`,
  `PubkeyAcceptedAlgorithms`.
- mosh: `--ssh=` (carrying the ssh subset), `-p`/`--port`, `--server=`, `--predict=`,
  `-a`, `-n`, `--no-init`, `-4`, `-6`, and
  `--experimental-remote-ip=local|remote`, `remote` by default.
