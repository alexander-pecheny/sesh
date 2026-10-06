---
status: proposed
---

# The transcript helper speaks tmux and zellij as well as herdr

Many Hosts run their Agents in tmux or zellij and have no herdr at all, and ADR 0006 still
left Projects useless there. So `sesh-transcript` gains a multiplexer layer with three
backends, herdr, tmux and zellij, each answering the same four questions: which panes run
an Agent, what a pane's screen shows, how to send it text and keys, and how to start an
Agent in a folder. The parsers, the protocol and the Conversation stay as they are.

A pane learns its Transcript and state from hooks, as herdr's do. On first connect the
helper installs its own: Claude and Codex hooks and a pi extension, each writing the pane
(`$TMUX_PANE`, `$ZELLIJ_PANE_ID`), session id, Transcript path, state changes and
permission prompts under `~/.sesh/agents/`. That gives exact matching, live state and
permission cards on any multiplexer. An Agent started before the hooks existed is matched
to the newest Transcript in its pane's folder, with its state read off the screen.

A Workspace is the multiplexer's top-level group: a herdr workspace, a tmux session or a
zellij session. Projects lists Agent sessions from every multiplexer on the Host and starts
new ones in herdr if it is there, else tmux, else zellij. A Host with none is refused with a
hint to install tmux, since an Agent session must outlive the SSH connection.

## Considered options

- Matching panes to Transcripts by folder and time alone, with no hooks. Nothing to install
  in the Agents' settings, but two Agents in one folder are told apart by guesswork, state
  comes only from the screen, and permission cards disappear. Kept only as the fallback.
- A Workspace as a tmux window or a zellij tab. Closer to herdr's tabs, but a Host with one
  long-lived session and many windows would show dozens of one-pane Workspaces. Rejected.
- A per-Host setting for where Start puts new Agent sessions. More control, but another
  field on the Host form for a choice the installed multiplexers already make. Rejected for
  now.

## Consequences

The helper edits `~/.claude/settings.json` and `~/.codex/hooks.json` on every Host Projects
opens, as herdr's integration does, and Codex asks once to trust the new hook. The
glossary's Workspace widens from herdr's group to the multiplexer's.
