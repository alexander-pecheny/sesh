---
status: accepted
---

# Sesh starts the follower on demand, and it follows every Agent on its machine

The Session log (ADR 0010) needs one process per machine that follows Agents whether or not
a device is looking. Sesh starts it: when a device connects to a machine it runs
`sesh-transcript serve`, which starts the follower in the background unless a lock file
shows one already running. The follower outlives the connection and exits after hours with
no Agent to follow, so the version Sesh pins is the version that runs, and a Host needs
nothing installed. After a gap the follower first catches up from the Transcript, which is
complete; only the provisional screen rows of the gap are lost, and the Transcript has
replaced those anyway.

It follows every Agent herdr runs on the machine, Unfiled ones included, so adopting a
session brings its whole log, and the sidebar marks and background work come from it too.
An idle session costs a watched file; the screen is read every 100 ms only while its Agent
works.

## Considered options

- herdr starting the follower with its server. Nothing missed between connections, but the
  helper's release would be tied to herdr's, and stock herdr could not run it. Rejected.
- A systemd or launchd service. The most robust, but an install step and an upgrade path
  on every Host. Rejected.
- Following only sessions a Vault has adopted. Less work, but the follower would need every
  Vault's records across machines, and an adopted session would start with a gap. Rejected.
