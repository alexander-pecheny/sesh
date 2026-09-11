# Sesh

Sesh is a native iOS terminal that reaches remote machines over SSH or mosh. Nothing runs
locally: every terminal is a window onto a remote shell. The terminal itself is
[libghostty](https://ghostty.org) — the real grid, renderer and escape-sequence handling —
driven in manual I/O mode, so bytes arrive from a transport instead of a child process.
SSH and mosh both live in one Rust core behind a C ABI. See `CONTEXT.md` for the
vocabulary, `docs/PLAN.md` for the plan and `docs/adr/` for the decisions.

Build with `just build` and install on the booted simulator with `just run`; both need
`xcodegen`, `just`, `zig` and Xcode 26.3. `just ghosttykit` builds the pinned
`GhosttyKit.xcframework` from the cmux fork with `patches/libxev-ios-async.patch`, which
restores libxev's cross-thread wakeup on iOS; without it libghostty's renderer and IO
threads never wake. `just ghosttykit-prebuilt` fetches the published build instead and
verifies it against `scripts/ghosttykit.sha256`, but that one is unusable on iOS. `just
gen` regenerates the Xcode project from `project.yml`, which is the only place project
settings are edited. Bundled JetBrains Mono is under the SIL Open Font License, copied
into `Resources/Fonts` with its licence.

`just core` builds the Rust crate in `core/sesh-core` — russh for SSH, the Extra flags
parser and the known-hosts check — for device and simulator and wraps it as
`Frameworks/SeshCore.xcframework`; `cbindgen` writes `Sesh/sesh.h`, which Swift reaches
through `Sesh/Sesh-Bridging-Header.h` rather than a module map, because GhosttyKit's
module map claims every header in the shared `include/` directory. `just core-test` runs
the crate's tests. `just sshd` starts an unprivileged sshd on 127.0.0.1:2222 with a
throwaway host key and test key under `.local/`, and `just sshd-stop` stops it; that is
what the simulator connects to.
