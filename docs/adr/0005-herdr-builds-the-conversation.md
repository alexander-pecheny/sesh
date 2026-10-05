# herdr builds the Conversation on the Host

A Conversation shows an Agent session as chat. Claude, Codex and pi each write their
Transcript in a different JSONL format, and only herdr knows which Agent runs in which
pane and where its Transcript is. So the owner's herdr fork parses all three on the Host
and streams one shared set of entries, with the Agent's state, from
`herdr agent follow`. Sesh renders entries and never learns which Agent wrote them. Projects
therefore requires the fork: on any other herdr it refuses and says what to install.

The Agents still run as ordinary terminal programs in herdr panes, so Remote Control,
the owner's own `herdr` view and survival across the phone sleeping all keep working.
Questions and permission prompts are answered by pressing keys in the pane, after which
herdr checks the screen.

## Considered options

- Parse in Swift on the phone. Testable, and needs no fork change, but every Agent
  format change would wait for an App Store release, and the phone would download whole
  tool outputs. Rejected.
- Parse in the Rust core. Each entry type would cross the hand-written C ABI. Rejected.
- ACP, as Zed does. Typed structure, but the sessions would be Agent SDK sessions that
  herdr, Remote Control and the terminal cannot see. Rejected in September 2026.
- A blocking permission hook that waits for Sesh's answer. No key presses, but the
  terminal and the Claude app see no prompt while it waits, and a sleeping phone hangs
  the Agent. Rejected.

## Consequences

`follow`'s output is a protocol between an App Store build and a herdr the owner updates
on their own schedule. It carries a number; Sesh refuses one it does not know, and
draws any entry kind it does not know from the summary text every entry carries.
