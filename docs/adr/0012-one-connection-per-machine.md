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

Once sends and choices go through the follower, a device no longer runs herdr commands at
all; herdr stays the follower's business.
