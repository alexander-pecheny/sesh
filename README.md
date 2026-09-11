# Sesh

Sesh is a native iOS terminal that reaches remote machines over SSH or mosh. Nothing runs
locally: every terminal is a window onto a remote shell. The terminal itself is
[libghostty](https://ghostty.org) — the real grid, renderer and escape-sequence handling —
driven in manual I/O mode, so bytes arrive from a transport instead of a child process.
SSH and mosh both live in one Rust core behind a C ABI. See `CONTEXT.md` for the
vocabulary, `docs/PLAN.md` for the plan and `docs/adr/` for the decisions.

Build with `just build` and install on the booted simulator with `just run`; both need
`xcodegen`, `just` and Xcode 26.3. `just ghosttykit` downloads the pinned
`GhosttyKit.xcframework` from the cmux fork's releases and verifies it against
`scripts/ghosttykit.sha256`; `just gen` regenerates the Xcode project from `project.yml`,
which is the only place project settings are edited. Bundled JetBrains Mono is under the
SIL Open Font License, copied into `Resources/Fonts` with its licence.
