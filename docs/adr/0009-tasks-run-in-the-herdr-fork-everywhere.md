---
status: accepted
---

# Tasks run their Agent sessions in the herdr fork, on every machine

A Task's Agent sessions must outlive Sesh, on the Mac as much as on a Host, and the Mac
app needs the exact Transcript paths and permission prompts that only the owner's herdr
fork provides. So Tasks use the fork alone, on the Mac and on every Vault Host. Sesh
installs it as `~/.sesh/bin/herdr-sesh`, from a release build checked against a SHA-256
the app carries, and leaves any herdr the user installed untouched.

herdr-sesh runs a server of its own, the herdr session named `sesh`, apart from the
user's everyday one. Sesh never depends on which herdr, stock or fork, the user runs, and
the user can still look at Sesh's Workspaces with `herdr session attach sesh`. Each Task is
one Workspace there, named after the Task.

The first time Sesh starts its server on a machine where a herdr server already runs, it
first hands that server over to herdr-sesh with herdr's live handoff, which keeps every
pane's process, id and Workspace. Agent sessions started before herdr-sesh carry on in
Sesh's server under the same pane ids. The move takes the whole server, Workspaces the
user opened by hand included, and happens only once. If the handoff fails, Sesh does not
start a server there, because a fresh one would hand out the same pane ids that its
records already name; it asks the user to stop the old server instead. A newer herdr-sesh
takes over the running one by the same handoff.

The phone follows the same model as the Mac. Its main screen is the Tasks list, its
Projects and Terminal modes are retired, and every shell it opens is a Terminal Tab in a
Task, attached to a herdr pane, so nothing runs outside a Task. Sessions started in
another multiplexer are found for Unfiled by the helper; tmux and zellij wait for ADR 0007.

## Considered options

- Bundling the fork in Sesh.app. Allowed on macOS and always the right version, but about
  20 MB in every app update and no help on the Hosts. Rejected.
- Whatever multiplexer each machine has, through the helper. Works on more machines, but
  means two code paths from the first build and no permission prompts. Rejected for now.
- The user's everyday herdr server, with a private one only where a stock herdr holds it.
  Agents show up in the herdr the user attaches to, but Sesh would depend on whatever
  herdr the user upgrades to, and run two kinds of server. Rejected.
- Keeping Agent sessions started before herdr-sesh in the user's server, with a second
  code path to reach them. Rejected for the one-time handoff.

## Consequences

The fork needs release builds for macOS and Linux on a public mirror, as the helper has
(`scripts/release-herdr.sh`). Agents moved by the handoff keep the old server's socket in
their environment, so their hooks no longer report to herdr until they restart; Sesh then
reads their state off the screen. Agents in the user's own herdr server, started after the
move, do not appear in Unfiled.
