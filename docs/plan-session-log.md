# Session log: plan

The design is in ADRs 0010 to 0013. The work runs in four stages, each usable on its own;
the old Conversation keeps working until the last.

1. The follower and the Session log, with recorded tests. No app changes. Done.
2. The native list on the Mac, reading the Session log, behind a switch in Sesh Dev. Done.
3. The same on the phone, sharing the row model and scroll rules. Done; the phone still
   renders Markdown with MarkdownUI, so one shared renderer remains to do.
4. Removal of the per-Conversation `follow`, the live overlay, the SwiftUI chat and the
   `agent list` and `background` polls. Done; `follow` stays for Transcript copies read
   from a Vault, which have no pane.

## Stage 1

### The follower

`sesh-transcript serve` takes an exclusive lock on `~/.sesh/follower.lock`, detaches, and
listens on `~/.sesh/follower.sock`. A second `serve` finds the lock and exits at once. It
exits by itself after six hours with no Agent running and no device attached.

One loop drives every session:
- `herdr agent list` every 250 ms gives each pane's Agent, state and session.
- Each session keeps the parsing state `follow` keeps today (`Follower`, `Live`), so the
  Transcript parsers and the screen reader move over, not get rewritten.
- A session's Transcript is read when the file grows; its screen is read every 100 ms only
  while its Agent works, and for three seconds after.
- Background work comes from the same Transcript reads that `background` does now.

### The Session log

`~/.sesh/sessions.db`, in SQLite as the Vault is:

| Table | Holds |
|---|---|
| `sessions` | key, pane, Agent, current Transcript, state, status line, marks, background work, seq of last change |
| `items` | session key, id, order, seq, final, kind, the entry as JSON |
| `meta` | the machine's sequence counter |

An item from the screen is written with `final = 0`. When `Live::deliver` matches a
Transcript entry to it, the same row is rewritten with the entry and `final = 1`; an entry
with no screen item is written straight as final. A screen item the Transcript never
matches, such as text Claude rewrote as narration, is dropped when its turn's entries have
all arrived, which is the one deletion the log knows, sent as a tombstone so devices drop it
too. Every write bumps the counter and stamps the row.

The session key is herdr's pane id for now, since Sesh's records point at panes. When a
session moves to another Transcript after a clear or resume, a `switch` item marks the place,
as the Conversation does today.

### The connection

`sesh-transcript attach` starts `serve` if no socket answers, then joins stdin and stdout to
the socket. Requests and replies are JSON lines.

| Request | Reply |
|---|---|
| `{"op":"hello","protocol":3}` | `hello`, then a `session` line for every session |
| `{"op":"watch","session":K,"since":N}` | every item of K stamped after N, then live changes |
| `{"op":"unwatch","session":K}` | nothing more for K |
| `{"op":"page","session":K,"before":ORDER,"limit":L}` | up to L items before ORDER, then `page_done` |

The follower sends `session` lines whenever a summary changes, and `item` and `tombstone`
lines for watched sessions, each with its seq. Sends, answers and choices move onto this
connection in stage 4.

### The recorder and replay tests

`serve --record DIR` writes, per session, every input the follower read, in order with
timestamps: `herdr agent list` results, screens, and Transcript bytes appended.
`sesh-transcript replay FILE` runs the same follower over a recording on a simulated clock
and prints the items it would write. Tests replay recordings in
`core/sesh-transcript/tests/sessions/` and compare the items with an expected file, so a
change in matching shows as a diff.

Recordings made for the bugs we know, on scratch sessions:
- a reply shown twice when a follow resumes;
- a slash command written twice under one prompt;
- a message queued during a compaction;
- interrupt and send now, which used to leave a message twice;
- a menu no hook reports;
- a long reply with tables streaming in;
- a Transcript that stalls after a compaction.

### Done when

- Replay tests pass for every recording above, with no item shown twice.
- `serve` followed every Agent on this Mac and on vps-he for a day without growing in memory.
- `attach` from a shell gives the summaries and, for a watched session, its items within
  100 ms of the screen showing them.
