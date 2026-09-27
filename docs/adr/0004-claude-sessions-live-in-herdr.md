# Claude sessions live in herdr

A Claude session must outlive the SSH connection that started it, and interactive
`claude` needs a terminal to run in. Projects starts each one as a herdr agent: the
folder becomes a herdr workspace, the Claude session a pane in it, started with
`herdr agent start <name> --kind claude`. A new branch comes from `herdr worktree
create`, so herdr links the copy to the original folder.

herdr already knows which panes hold agents and whether each is idle, working, blocked
or done. Projects reads that state from `herdr agent list` instead of inventing its own,
and `agent prompt`, `read` and `wait` are the first foothold for a chat UI. The owner
runs herdr everywhere, so a Claude session started from Projects can be opened by the owner from
a Terminal Tab with `herdr`.

## Considered options

- Detached tmux, one session per Claude session. Works everywhere and can be attached,
  but knows nothing about agents: status would mean scraping panes ourselves. Rejected.
- `systemd-run --user`. Survives more, but gives Claude no terminal, needs linger, and
  cannot be attached. Rejected.
- `claude remote-control --spawn worktree`, one server per folder. Sesh would create
  folders but never Claude sessions, and would hold no handle for a later chat UI.
  Rejected.
