---
status: accepted
---

# A sent message is a Session log item from the moment it is sent

A message used to show only once the follower read it back off Claude's screen or the
Transcript, half a second to two seconds after the user sent it. Guessing on the device which
row was the sent one failed in turn on every case: Claude shows a long paste as "[Pasted text
#1 +12 lines]", the Transcript gives its entry an id of its own, a page of history brings older
messages, and two quick sends look alike. Now the device names each message, as `sent.` and a
UUID, and passes that id with the text on `send`. It shows the message at once under that id,
dimmed, and the follower writes an item of the same id to the Session log, which takes the
device's row over. The follower matches the text to Claude's screen and to the Transcript
entry, once, and rewrites the same item; a failed send drops it, and the device puts the text
back in the input box.

The follower also holds the queue, so every device shows the same one and none needs to be
open for it to go. An item's state says where the message stands:

- queued: Sesh holds it, since pi takes no message while it works. `unqueue` takes it back.
- handed: given to the Agent while it works; Claude holds it and reads it at its next step.
- sent: given to an idle Agent, and not yet shown on its screen.
- shown: Claude shows it as read. Codex and pi go from sent straight to final.
- lost: given to the Agent, which sat idle for ten seconds without showing it or writing it to
  the Transcript. The device offers to send it again or take it back. A slash command goes
  instead, since Claude shows commands such as /clear in a form of its own, or not at all.
- final: the Transcript holds it, as with every other item.

Once the Agent is neither working nor blocked, the follower hands the queue over as one
message, as the Agent gets it; `hand` does the same at once, for Claude's "Send now".

## Considered options

- Matching on the device. No change to the follower, but every device guesses on its own and
  each guess broke a case above. Rejected.

## Consequences

A message that waits has no place in the order until the Agent takes it: it moves below the
rows that land before it, and then goes after every row but the messages still waiting. One
sent to an idle Agent is already there, so its row keeps its place from sent to final. A
message written while its Agent starts is held on the device and sent once the Agent is up.
