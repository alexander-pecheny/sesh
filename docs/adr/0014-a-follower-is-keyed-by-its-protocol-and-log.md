---
status: accepted
---

# A follower is keyed by its protocol and its log, and a newer build takes it over

Each helper version used to run a follower of its own under `~/.sesh/follower/VERSION`, where
the version was a hash of every source byte. Any edit, a test or a comment included, meant a
release and a pin, then a new follower with an empty Session log beside the old one, every
device's cached Conversations dropped, and the old follower running on for hours. Now the
folder is named by the follower's protocol and its Session log's schema, as in `p4-s1`, and
every build that shares both runs the same follower over the same log. A change to what
devices and the follower say to each other bumps the protocol; a change to the log's tables,
or to what their rows mean, bumps the schema, which the log also records and checks on open.

When `serve` finds a follower from an older build in its folder, it asks it to leave over the
socket, waits until it hangs up, and starts its own over the same log. The old follower first
stops taking acts, plays and answers those it took, for a few seconds at most, and a device whose
act it refuses in that time sends it again to the new one. An older build finds
a newer follower and simply uses it, since they speak the same protocol. Builds are ordered by
the UTC time `build-helpers.sh` stamps into them, not by their hash, so two devices that pin
different builds both end up on the newer one instead of taking it over in turn.

## Considered options

- Hashing only the shipping sources, and keeping a follower per version. Fewer releases, but
  every real change still starts an empty log and drops every device's cache. Rejected.
- Signalling the old follower by the pid in its lock file. As short, but a reused pid could
  name another process. Rejected.

## Consequences

A device keeps its cached Conversation across helper builds with the same pair and resumes
from the last item it holds. A dev build is numbered 0 and never takes over a released one.
