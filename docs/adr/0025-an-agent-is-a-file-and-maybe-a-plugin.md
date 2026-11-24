---
status: accepted
---

# An Agent is described by a file, and a plugin reads what the file cannot

Sesh knew Claude, Codex and pi through code spread across sesh-mux, the helper and the apps. A
fourth Agent meant a fork, and an Agent that must stay private meant a private fork that drifted
from this repository.

Each Agent is now a TOML Agent file. sesh-mux, the helper and the apps all read the same file,
each taking the parts it needs:

- The command that starts the Agent, other names its process goes by, the arguments that start,
  resume or skip permissions, and the shell test that says whether it is logged in.
- Where its Transcripts are, what their file names look like, what their first line says, and
  where a placed Transcript goes and which fields name its folder and session.
- The keys that answer its permission and question menus, and the screen rules sesh-mux uses to
  tell whether it is working, idle or blocked.
- For an Agent without hooks, the file its process writes naming the session it holds. sesh-mux
  reads that file once a second.

Claude's, Codex's and pi's files are in `mux/agents`, inside the tree that versions sesh-mux.
The helper compiles them in too. The apps carry a copy in `Resources/agents.json`, which
`just agents` writes and a test keeps in step. Their Transcript parsers stay in Rust, named by
the file. A file in `~/.config/sesh/agents` on a Host adds an Agent or replaces a built-in one.
`sesh-transcript agents` lists what a Host knows, and the apps learn and remember it each time
they reach the Host.

When reading an Agent needs code, its file names a plugin. The helper starts one copy of it per
Host, the first time it is needed, and keeps it running, because a Host may follow dozens of
Agents at once and a process for each would double their number. The helper sends it one JSON
object per line, each tagged with the Transcript stream it belongs to, and the plugin answers each
with one line: the entries a Transcript line makes, or the menu a screen shows. JSON lines cost
tens of microseconds a line against a 100 ms poll, and any language can speak them. A plugin that
dies is started again and has lost its streams.
