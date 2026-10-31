---
status: accepted
---

# The Mac reaches Hosts through the core, reading the owner's ssh config itself

The Mac used to run every command, stream and Terminal through `/usr/bin/ssh`, with a
ControlMaster per Host, and its Terminals through a bundled `mosh` that started yet another
ssh. That kept two ways of reaching a machine: a fix to the core's Link, such as reading the
login shell's environment once, never reached the Mac, and a Host's load came mostly from the
Mac's login shells. Now the Mac holds one core Link per ssh alias, as the phone holds one per
Host, and both share `CoreLink`.

The core reads `~/.ssh/config` and `/etc/ssh/ssh_config` as ssh(1) does: `Host` and `Match`
blocks (`exec`, `final` and `canonical` included), `Include`, first value wins, tokens and
hostname canonicalization. It dials directly, through a `ProxyCommand` run in the user's login
environment (so `aws ssm start-session` works from an app started from the Dock), or along a
`ProxyJump` chain; signs in with the agent's keys, then the identity files, asking for a
passphrase where needed; and checks host keys against the user's `known_hosts` files, hashed
and wildcard entries and `@revoked` included, under `StrictHostKeyChecking`.

A Terminal runs on the Link's connection: over mosh, started on that connection, where the Host
is reached directly, and on a pty channel behind a proxy, where mosh's datagrams cannot follow.
Pipes do the same. Every mosh-server carries the device's and the app run's tag, and a new run
stops that device's servers from earlier runs.

## Considered options

- Keeping system ssh as a fallback for what the core cannot do. Every fallback is a second
  code path whose failures look like the first's. Rejected.
- Resolving each alias with `ssh -G`. Exact, but it needs system ssh on every Mac and runs a
  process per Host. Rejected.

## Consequences

An option the core does not act on is read and ignored, as ssh(1) ignores those that do not
apply to it; `Match localnetwork` fails the connection with a message instead. A certificate
whose key only the agent holds is not offered, since russh signs certificates only with keys it
holds itself.
