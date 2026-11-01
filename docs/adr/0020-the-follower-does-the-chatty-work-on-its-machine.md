---
status: accepted
---

# The follower does the chatty work on its machine, and talks to sesh-mux over its socket

Two jobs cost a device many round trips, each a new exec channel with its own quoting. Copying
Transcripts into a Vault took four or five per session every 30 seconds: `sesh-mux pane get`,
`vault size`, `tail -c | base64`, a put, and `vault append`. Starting an Agent read its screen from
the device every 100 ms to get past the menus some Agents open before their first prompt. Both now
go to the follower (ADR 0011), which already runs on every machine and answers one-shot requests.

A copy round asks the follower on each machine the Vault's Agent sessions run on once, with
`transcripts`, and the Vault's follower once, with `store`:

- `transcripts` names each session's pane, its Agent's name and folder, the Transcripts it is known
  to have, and how long the Vault's copy of each is. The follower answers with the pane's current
  Transcript, only if the pane still runs the session's own Agent, since sesh-mux gives a closed
  pane's id to the next one, and with each Transcript's bytes past its copy, up to 4 MB, in base64.
  Given the Vault's folder instead, as when the Vault lives on the same machine, it copies them
  into the Vault itself.
- `store` appends those chunks to the Vault's copies, each at the offset it was read from, and
  answers with the length of every copy of the sessions named. The device keeps these lengths, so
  a round with nothing new costs one request per machine and none to the Vault.

`start` names the pane, the Agent and its arguments, and the Claude config folder if any. The
follower plays it on the pane's own thread like any act: sesh-mux starts the Agent, the follower
answers the menus it knows, and replies once the Agent can take a prompt, or waits on a menu Sesh
does not know, which the user answers in the Conversation. On failure it closes the pane and says
why, with the end of the screen.

A request too long for a command line, as a `store` is, goes up as a file that `sesh-transcript
ask --file` reads and removes. None of this changes what devices and followers already say to
each other, so the protocol stays 5: an older follower answers the new ops with an error until a
newer build takes it over (ADR 0014).

The follower asked sesh-mux for everything by running its CLI, a process per pane list, screen
read and key. It now makes the same requests over sesh-mux's socket, one per connection, as the CLI
does. sesh-mux announces a session id or a permission prompt a hook reports with `pane.updated`, so
the follower lists panes only when an event says something changed, not every 2 seconds as well.

## Considered options

- Sending the chunks to the Vault over the follower link a device keeps open (ADR 0012). One hop
  less, but that link rides mosh on the phone, which crawls under megabytes. Rejected.
- Keeping the start-up loop on the device, polling over a long-lived connection. Fewer channels,
  but still a round trip per screen read. Rejected.
