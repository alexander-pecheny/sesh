---
status: accepted
---

# Tasks run in sesh-mux, Sesh's own terminal server cut down from the herdr fork

Sesh ran its Tasks in a fork of herdr, a terminal multiplexer whose server, CLI and full
terminal interface came to some 150,000 lines of code, of which Sesh used the server alone.
sesh-mux keeps only that: workspaces, tabs and panes that outlive every client, worktrees,
Agents started, detected and resumed, their hooks, the socket API with its events, persistence
and live handoff, and a CLI that lists them and attaches one pane to a terminal. It lives in
this repository under `mux/`, keeps herdr's Apache-2.0 licence with a NOTICE naming it, and
says "herdr" nowhere else, so it is never mistaken for the user's own herdr.

Everything of it sits under `~/.sesh/mux/`, its variables are `SESH_MUX_*`, and its hooks are
`sesh-mux-agent-state.sh`, beside herdr's, which the user's own herdr still uses. New Worktrees
go in `~/.sesh/worktrees`.

A Host still running herdr-sesh hands its panes to sesh-mux by live handoff the first time a
new app reaches it, Agents running on. Those Agents keep herdr's variables and hook, so sesh-mux
also listens on herdr-sesh's socket while any pane from it lives, and `herdr-sesh` becomes a
link to `sesh-mux`, which ignores `--session`, for older apps and helpers.

## Considered options

- Trimming the fork in place. It ends upstream merges just the same, and keeps the fork's name
  and its separate repository. Rejected.
- Taking over a user's own herdr server on a Host's first run, as herdr-sesh did. It depends on
  upstream's snapshot format never changing. Dropped: a new Host starts empty.

## Consequences

Fixes upstream herdr makes to Agent detection and hooks, as Claude and Codex change, are ported
by hand.
