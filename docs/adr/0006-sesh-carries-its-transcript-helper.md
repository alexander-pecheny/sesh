---
status: accepted, supersedes the "Projects requires the fork" half of ADR 0005
---

# Sesh carries its own transcript helper

Projects often meets Hosts that run stock herdr, not the owner's fork, and refusing them
left Projects useless there. So the parsers that turn Transcripts into Conversation
entries move out of the herdr fork into `sesh-transcript`, a small static binary built
from Sesh's `core`. Sesh carries builds for Linux x86-64, Linux arm64 and macOS, and on
connect copies the right one to `~/.sesh/bin` over SFTP when it is missing or stale.

The helper speaks the same protocol `herdr agent follow` did, so the Conversation is
unchanged. It asks stock `herdr agent list` for each pane's state and session id, finds
the Transcript by that id, and answers questions and permission prompts with
`herdr agent send-keys`. Sesh uses it on every Host, fork or not; the fork only adds
exact Transcript paths and the permission hooks that make permission cards appear.

## Considered options

- Upstreaming `follow` into herdr. The cleanest end state and still worth offering, but
  it waits on the maintainer. Not instead of this.
- Reading the screen, as Moshi appears to. Works with tmux too, but loses diffs, tool
  cards and paging. Rejected.
- Uploading the fork's whole herdr binary as a client of the Host's server. About 20 MB
  per platform, and tied to herdr's socket protocol, which changes between versions.
  Rejected.
