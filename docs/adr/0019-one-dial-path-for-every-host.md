---
status: accepted
---

# Every Host is dialed from one spec, and every Terminal opens on its Link

After ADR 0017 the core had two ssh stacks. The Mac dialed an alias through `dial.rs`, with
everything its ssh config could say. The phone dialed a Host from its list through `ssh.rs`,
`mosh.rs` and its own `link::Config`, with a second authentication loop, a second keepalive and
host key check, and a fresh connection for every Terminal and every mosh Upload. A fix to one
stack, such as the keepalive or a proxy that works, never reached the other.

Now there is one `dial(spec)`, and the spec is `ssh_config::Host`: what ssh(1) would use to
reach one alias. Two adapters build it. The Mac resolves an alias from `~/.ssh/config`. The
phone builds it from its Host record in `flags::spec`: address, port and user; its Key, held in
memory, and its saved password, both from the Keychain; its own `known_hosts` file under
`StrictHostKeyChecking ask`; and its Extra flags as the options they stand for, so `-p`, `-l`,
`-4`, `-6`, `-J`, `-A`, `ServerAliveInterval`, `ServerAliveCountMax`, `HostKeyAlgorithms` and
`PubkeyAcceptedAlgorithms` become `Port`, `User`, `AddressFamily`, `ProxyJump`, `ForwardAgent`
and the rest. A jump hop of a phone Host is reached as the Host itself is, at the hop's address.
The spec carries what only the phone has: Keys in memory, tried after the agent's and before the
identity files; a saved password, tried once before the user is asked and reported when refused;
and an agent made of the phone's own Keys.

A phone Terminal opens on its Host's Link with `sesh_session_terminal`, as a Mac Terminal
does: over mosh where the Host is reached directly and its Transport is mosh, on a pty channel
otherwise. The mosh Extra flags `--server`, `--port`, `--predict` and
`--experimental-remote-ip` reach that terminal. Uploads go over the Link too. Mac and phone
share one `Machine`, whose only Mac part is running commands on the Mac itself.

## Considered options

- Translating the phone's record into an ssh config text and resolving that. It round-trips
  through a parser to get back a struct the phone could build directly, and the Keychain's
  secrets have no place in a config file. Rejected.
- Keeping a connection per phone Terminal. It costs an authentication per Tab and keeps the
  second stack alive for no gain: the Link is already up whenever a Terminal can be opened.
  Rejected.

## Consequences

A phone Host reached through `-J` now has a Link; its Terminals run on a pty channel there,
since mosh's datagrams cannot follow a jump. Agent forwarding is asked for on a pty terminal's
channel, where a shell can use it; a mosh terminal's server outlives the channel that started it,
so it never can. `-t` and `--no-init` are accepted and change nothing, since every terminal
already has a pty and Sesh never acted on `--no-init`. A Terminal on a dropped Link ends and
offers Reconnect, which dials the Link again.
