---
status: accepted
---

# sesh-mux and each Vault say what they are, and a push can be sent twice

Sesh kept sesh-mux and the helper compatible only by pinning both in the same app build. The app
noted the sesh-mux version it installed in a file beside it, so a Mac and a phone that pinned
different builds each found the other's and put their own back, handing the server over every
time. A helper that read a field sesh-mux no longer gave failed quietly. A device knew an erased
Host only by its counter being behind, so a Host made again and refilled past that counter went
unnoticed. A push whose answer was lost was sent again and made a conflict copy of the device's
own edit.

- sesh-mux reports its socket API version in `ping` and in `--version`, beside the UTC time
  build-mux.sh stamps on it. The helper pings before its first request, and again after the socket
  fails or the event stream ends, as at a handoff; on another API it refuses, saying which side to
  update. A sesh-mux that names no API speaks API 1, the API from before it said so.
- The installer asks the installed sesh-mux its build and replaces it only with a newer one, so a
  device never puts back an older build another device installed. A pin from before builds were
  stamped still goes by the noted version.
- `vault init` gives each Vault a random id, which every head line carries. A device keeps it with
  its cursor; another id means the Vault was made again, and the device refills it as it refills
  an erased one. A refill also sends a record the Host holds an older edit of, since the other
  device that refilled it first may have had an older copy.
- A device sends its queue as a batch under an id it keeps until the Host answers. The Host
  remembers its answer to the last 1000 batches and gives the same one to a batch sent again. A
  push that fails is tried again after 2 seconds, doubling up to a minute.
- A push answers with the record a losing edit left standing, and a device keeps the newest record
  follow brought for an id it had queued, so its copy ends equal to the Host's.

None of this changes what devices and followers say to each other over the follower link: the head
line gains a field older devices ignore, so the follower protocol stays 5 (ADR 0014).

## Considered options

- One sesh-mux path per version, as the helper has. Two servers would each think they own the
  panes. Rejected.
- Checking sesh-mux's API on every request. A ping per request doubles the socket traffic the
  follower makes. Rejected.
- Making each change idempotent by its base alone. A lost answer leaves the device unable to tell
  its own write from another device's. Rejected.

## Consequences

When two devices refill a Host made again and both hold a record, the Host may keep a conflict
copy of the older text. A Vault made again while a device stays connected is refilled only once
that device reconnects.
