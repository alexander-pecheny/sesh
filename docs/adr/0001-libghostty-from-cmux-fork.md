# libghostty comes from the cmux fork, not upstream

iOS cannot fork a child process, and upstream libghostty (v1.3.1, September 2026)
has only the Exec termio backend, which does. The cmux fork
(github.com/manaflow-ai/ghostty) adds `GHOSTTY_SURFACE_IO_MANUAL`: the embedder pushes
terminal output in and receives keystrokes through a write callback. We build
GhosttyKit.xcframework from that fork, pinned by commit.

## Considered options

- Upstream plus a vendored ~380-line manual-I/O patch, rebased on every bump. Rejected:
  same code, more upkeep.
- Upstream `libghostty-vt` plus our own Metal renderer. Rejected: weeks of renderer work
  for a worse terminal.
