---
status: accepted
---

# A device keeps one two-way connection to each machine's follower

A device used to run one ssh command per open Conversation and poll `herdr agent list` and
the helper's `background` besides. Now the follower listens on `~/.sesh/follower.sock`, and
a device reaches it by running `sesh-transcript attach` over the ssh or mosh link it already
has, which joins its stdin and stdout to the socket. Over that one connection the device
asks to watch a session, to stop watching it, or for an earlier page, and later sends
messages and choices. The follower answers with a summary of every session on the machine,
for the marks, and with item changes for the watched sessions only, each stamped with its
sequence number, so a reconnect resumes from the last number seen.

## Considered options

- One stream carrying every change on the machine, with no requests. Simpler, but the phone
  would receive every working Agent's screen rows over mobile data. Rejected.

## Consequences

Both devices run their streams over mosh pipes, which carry stdin, so one `attach --sessions`
per machine also carries a `watch` or `unwatch` line for each session the device opens or
leaves. A stream that cannot be written to, as over plain ssh from the phone, names what it
wants on `attach`'s command line instead: `--watch KEY:SEQ` for each session it shows, from
the last item it has, so opening another session starts another `attach`. Earlier pages come
through a one-shot `page`. The
follower a device reaches is the one for its protocol and log, which a newer helper build
takes over (ADR 0014), so a device never talks to a follower that speaks another protocol.

Messages, keys, answers, permissions and stops go to the follower the same way, each through
a one-shot command (`send`, `keys`, `answer`, `permit`, `stop`), and the follower plays them
into the pane. `unqueue` and `hand` take back or hand over a queued message (ADR 0015). A device still runs herdr itself only to make Workspaces, Worktrees and panes,
to start an Agent, and to attach a Terminal.
