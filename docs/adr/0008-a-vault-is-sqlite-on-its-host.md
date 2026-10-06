---
status: proposed
---

# A Vault is a SQLite database on its Host, copied to every device

A Vault holds a user's Tasks, Journals, Documents and the Transcripts of every adopted
Agent session, and both the Mac and the phone must reach it. So a Vault lives on one Host
as a SQLite database, with Transcript copies kept as files beside it, and Sesh's helper on
that Host is the only program that writes them. Each Mac and phone keeps a full copy of the
database, without the Transcripts, which it fetches when a Conversation opens.

The Host's database is the truth. A device writes to its own copy at once and queues the
change, so Entries and Documents can be written offline; on reconnect the queue goes to the
Host and the device takes what changed elsewhere. Entries and Documents merge one by one:
new ones never conflict, and when two devices edit the same one, the later edit wins and
the other text is kept as a conflict copy beside it. A Document that lives on a Host as a
file in a worktree is not in the database; the Vault keeps only its last-seen text, for
reading while the Host is off or the file is gone.

## Considered options

- Plain Markdown files in a folder on the Host. Greppable and easy to back up with git,
  but search, ordering and offline merging would all be built by hand on top of files.
  Rejected.
- A local Vault on the Mac synced through iCloud or CloudKit. No server work, but the
  Hosts and their Agents could never read a Task, and the phone would depend on Apple's
  sync. Rejected.
- Merging edits character by character with a CRDT. Two devices editing one Document
  would both survive inline, but the machinery is large and such conflicts are rare
  between one person's devices. Rejected for now.

## Consequences

A Vault is unreadable on a new device until it reaches the Host once. The Vault's Host
needs disk for every Transcript its Tasks ever adopted.
