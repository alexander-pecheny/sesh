---
status: proposed
---

# Tasks run their Agent sessions in the herdr fork, on every machine

A Task's Agent sessions must outlive Sesh, on the Mac as much as on a Host, and the Mac
app needs the exact Transcript paths and permission prompts that only the owner's herdr
fork provides. So, for now, Tasks use the fork alone, on the Mac and on every Vault Host.
Where it is missing, Sesh runs a shell script that downloads a release build, checks it
against a SHA-256 the app carries and installs it in `~/.sesh/bin`, leaving any herdr the
user installed untouched.

Sesh starts Agent sessions in the user's everyday herdr server, so they show up when the
user attaches from a terminal; each Task is one herdr Workspace named after the Task. If
nothing runs, Sesh starts that default server itself. If a stock or older herdr holds it,
Sesh uses a private server of its own rather than replace the user's.

The phone follows the same model as the Mac. Its main screen is the Tasks list, its
Projects and Terminal modes are retired, and every shell it opens is a Terminal Tab in a
Task, attached to a herdr pane, so nothing runs outside a Task. Sessions started in
another multiplexer are found for Unfiled by the helper; tmux and zellij wait for ADR 0007.

## Considered options

- Bundling the fork in Sesh.app. Allowed on macOS and always the right version, but about
  20 MB in every app update and no help on the Hosts. Rejected.
- Whatever multiplexer each machine has, through the helper. Works on more machines, but
  means two code paths from the first build and no permission prompts. Rejected for now.
- A private herdr server for Sesh everywhere. Isolated and predictable, but it hides the
  user's Agents from the herdr they attach to every day. Kept only as the fallback.

## Consequences

The fork needs release builds for macOS and Linux on a public mirror, as the helper has,
and its socket protocol must keep pace with Sesh releases.
