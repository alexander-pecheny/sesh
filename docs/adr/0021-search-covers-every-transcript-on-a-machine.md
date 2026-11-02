---
status: accepted
---

# Search covers every Transcript on a machine, and a Transcript can move to another

Search used to read only the Vaults, so an Agent session started outside Sesh, or never
adopted, could not be found without `sess`, a script that lives on one person's machines.

The helper now keeps an index of every Transcript under the home folder of the machine it
runs on: Claude's in each `~/.claude*` folder, Codex's and pi's. It sits in
`~/.sesh/transcripts.db` and catches up on the bytes each file gained before every search.
Like the Vault, it indexes what the user and the Agent wrote, not tool calls or their
output. Search asks the Mac and every Vault's Host and leaves out what a Vault already holds.

To resume a Transcript on another machine, the device moves the file over the connections it
already has, and the helper there puts it where the Agent looks, with each line's folder
changed to one that exists there. A fork does the same under a new session id. The folder
names follow atuin's Transcript writers.

## Considered options

- Atuin Threads as the index. It syncs and resumes across machines, but it has no API for
  other programs, and Sesh would depend on a daemon it does not control. Rejected.
- Translating a conversation to another Agent, as `atuin resume --in` does. Rejected for
  now: the move keeps the Agent, so the file needs only its folder changed.

## Consequences

The first search on a machine indexes everything, about five seconds for 1.5 GB on a Mac.
A resumed Transcript leaves its original where it was, so the two can diverge; moving it
back then forks it, rather than overwrite either.
