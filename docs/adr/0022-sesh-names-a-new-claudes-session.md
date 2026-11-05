---
status: accepted
---

# Sesh names a new Claude's session, and sesh-mux starts every Agent with its variables

The follower learned which Transcript a pane's Claude writes from sesh-mux's `SessionStart` hook.
When the hook was missing, sat in another account folder or had not run yet, it guessed: from
Claude's records of running sessions, the processes in `/proc` and the pane's folder. Each guess
failed somewhere, and `/proc` does not exist on a Mac.

A new Claude now starts with `--session-id` and a UUID the app picks. sesh-mux reads the id from
the launch arguments, as it already did for `codex resume`, and reads `--resume` the same way, so the pane names its session before
Claude writes a line. The hook still reports the id that `/clear` or a resume moves Claude to. A
pane that names no session gets no Transcript; the guessing is gone.

`agent.start` takes the Agent's variables and puts them on its command line under `env`. sesh-mux
always adds the pane's id and its socket, which a shell another server started may lack, and the
follower passes `CLAUDE_CONFIG_DIR` the same way. Nothing is typed into a shell before the Agent. The follower installs the hooks in Claude's folder before each start, because on a new Host that folder may appear only when a Transcript is copied in.

## Considered options

- Keying sessions by Claude's session id instead of the pane. Rejected: `/clear` gives the same
  Agent session a new id, and Codex and pi cannot be given one.
- Passing the hooks with `--settings` instead of installing them in each account folder. Rejected
  for now: a Claude the user starts in a Terminal still needs the installed hook.
