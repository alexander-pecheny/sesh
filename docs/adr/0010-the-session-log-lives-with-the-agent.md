---
status: accepted
---

# The Session log lives on the Agent's machine, one item per row, rewritten in place

Every device used to build a Conversation for itself, from two sources merged by guesswork:
the Transcript, which is complete but about five seconds late, and Claude's screen, which
is immediate but has no structure. A live row and its Transcript entry were two items, and
each device decided when one replaced the other, so messages showed twice, rows flickered
and the Mac and the phone disagreed. Now one process on the machine the Agent runs on makes
that decision once and writes the result to a Session log, which every device shows as is.

The Session log is a SQLite table of items, one per row. An item gets a stable id and a
fixed place in the order when it first appears, usually on the screen. When the Transcript
entry for it arrives, the same item is rewritten and marked final; nothing is removed and
added again, so no device can show both. Every write stamps the item with the machine's
next sequence number, and a device asks for everything stamped after the last number it
saw, as it already does with a Vault (ADR 0008). A screen row rewritten ten times a second
is one row, not ten events. The Vault keeps a copy of finished history for search and for
reading while the machine is off.

## Considered options

- The log in the Vault. One store per Vault, but a session on the Mac whose Vault is on
  vps-he would send every live row over ssh and back, on the fastest path in the app.
  Rejected.
- An append-only stream of "added" and "replaced" events. Closest to what existed, but
  every device must apply replacements correctly, and the stream grows with every redraw.
  Rejected.

## Consequences

Matching screen text to Transcript text still happens, but in one place, so a recorded
session replays to exactly the same items in a test. An item's place never changes after it
lands, so a message never jumps.
