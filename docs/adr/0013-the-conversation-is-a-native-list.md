---
status: accepted
---

# The Conversation is a native list with heights Sesh measures itself

The Conversation was a SwiftUI lazy stack holding an AppKit text view per message. The stack
guesses the heights of rows it has not drawn, and five rules on top of it tried to keep the
scroll in place; between them the chat went blank, jumped, paged back through history on its
own and broke "Move to bottom", on both platforms. Now it is an `NSTableView` on the Mac and
a `UICollectionView` on the phone, one row per Session log item. A row's height is measured
once and kept under the item's id, its revision and the width, and measured again only when
the item is rewritten or the width changes. Scrolling is Sesh's own code, shared by both
platforms: at the bottom, new content is followed; rows added above keep the row being read
in place; "Move to bottom" scrolls to the last row. One renderer turns Markdown into an
attributed string for both, so tables, code and digits look alike.

## Considered options

- A web view, as Claude's and Codex's desktop apps use. One renderer and the browser's own
  scroll anchoring, but not native. Rejected by choice.
- One TextKit 2 document for the whole Conversation, with cards as embedded views. Selection
  across messages, but cards, buttons and live rewrites inside one text view are much harder,
  and the platforms differ more there. Rejected.

## Consequences

Text can be selected within one message, not across messages, as before.
