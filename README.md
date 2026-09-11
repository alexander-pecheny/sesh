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

Bundled JetBrains Mono is under the SIL Open Font License, copied into `Resources/Fonts`
with its licence.

## Extra flags

A Host's Extra flags are parsed against a subset and anything else is refused before
connecting, naming the flag.

- ssh: `-p`, `-l`, `-4`, `-6`, `-J user@host[:port]` (one hop), `-t`, `-A`, and `-o` with
  `ServerAliveInterval`, `ServerAliveCountMax`, `Port`, `User`, `HostKeyAlgorithms`,
  `PubkeyAcceptedAlgorithms`.
- mosh: `--ssh=` (carrying the ssh subset), `-p`/`--port`, `--server=`, `--predict=`,
  `-a`, `-n`, `--no-init`, `-4`, `-6`, and
  `--experimental-remote-ip=local|remote`, `remote` by default.
