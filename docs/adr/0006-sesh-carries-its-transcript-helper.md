---
status: accepted, supersedes the "Projects requires the fork" half of ADR 0005
---

# Sesh carries its own transcript helper

Projects often meets Hosts that run stock herdr, not the owner's fork, and refusing them
left Projects useless there. So the parsers that turn Transcripts into Conversation
entries move out of the herdr fork into `sesh-transcript`, a small static binary built
from Sesh's `core`. `just helpers-release` publishes builds for Linux x86-64, Linux arm64
and macOS as a release on the GitHub mirror, and the app carries only `helpers.json`: the
version, the release and each build's SHA-256. On connect, when `~/.sesh/bin` holds another
version, the Host downloads its build with curl or wget and checks the hash; a Host that
cannot reach GitHub gets the same bytes through the phone over SFTP.

The helper speaks the same protocol `herdr agent follow` did, so the Conversation is
unchanged. It asks stock `herdr pane get` for each pane's state and session id, finds
the Transcript by that id, and answers questions and permission prompts with
`herdr agent send-keys`. Sesh uses it on every Host, fork or not; the fork only adds
exact Transcript paths and the permission hooks that make permission cards appear.

## Considered options

- Upstreaming `follow` into herdr. The cleanest end state and still worth offering, but
  it waits on the maintainer. Not instead of this.
- Reading the screen, as Moshi appears to. Works with tmux too, but loses diffs, tool
  cards and paging. Rejected.
- Bundling the builds in the app. App Store validation rejects macOS executables inside
  an iOS app, and a reviewer reading guideline 2.5.2 need not see that they never run on
  the phone. Rejected; the pinned hash keeps the same guarantee.
- Uploading the fork's whole herdr binary as a client of the Host's server. About 20 MB
  per platform, and tied to herdr's socket protocol, which changes between versions.
  Rejected.
