# Sesh

A terminal for iOS and the Mac that reaches remote machines over SSH or mosh. On the
phone nothing runs locally; on the Mac a Tab may also run on the Mac itself.

## Language

**Host**:
A saved way to reach one remote machine: address, user, transport, key, flags. On the
phone Hosts live in Settings, where any of them also opens a plain shell outside every
Task; the Mac names a Host by its ssh alias instead.
_Avoid_: server, connection, profile

**Key**:
An SSH private key the app holds, imported by the user. A Host names the Key it uses.
_Avoid_: identity, credential

**Transport**:
How a Host is reached: SSH or mosh. A Host has one Transport.
_Avoid_: protocol, mode

**Extra flags**:
Free-text options the user appends to a Host's ssh or mosh command line. Parsed
against a documented subset; anything else is rejected before connecting.
_Avoid_: custom command, arguments

**Remote command**:
An optional command a Host runs instead of the login shell, such as attaching to tmux.
_Avoid_: startup command, initial command

**Agent forwarding**:
A per-Host option under which the app answers the remote side's agent requests with its
own Keys. There is no separate agent. Applies to the SSH Transport only, since mosh's
SSH session ends once the server starts.

**Session**:
One live connection to a Host, shown in one Tab. Ends when the remote shell exits or the
user closes it.
_Avoid_: terminal, connection

**Tab**:
One thing open at a time in Sesh. Inside a Task a Tab is the Journal, a Document, a
Terminal or an Agent session, and every Tab but the Journal can move to another Task. A
Terminal is a shell in a herdr pane of the Task's Workspace: it outlives the app, and
closing its Tab ends it. An Agent session Tab shows the Conversation or the Agent's own
terminal. The phone has the same Tabs; its Terminal is an ordinary Session whose remote
command attaches the herdr pane.
_Avoid_: pane, page

**UI mode**, **Projects**:
Retired. The phone once opened a Host in a Terminal or a Projects Tab; its main screen
is now the Tasks, as on the Mac.

**Agent**:
The coding agent behind an Agent session: Claude, Codex or pi. Other coding agents running
on the Host are not Agents, and Sesh does not show them.
_Avoid_: model, tool, kind

**Agent session**:
One conversation with one Agent, running on the Host in one folder, optionally on its own
branch. It outlives every Session and Tab. When the Agent
clears or resumes, it moves to another Transcript but stays the same Agent session.
Always "Agent session" in full; a bare Session is the SSH or mosh connection.
Every Agent session belongs to one Task once adopted, and keeps belonging after
its Tab closes and its Agent exits. It can move to another Task but never back to Unfiled.
_Avoid_: Claude session, thread, agent pane

**Ended**, **Resume**:
An Agent session is ended once its Agent has stopped, whether the user ended it or the Agent
exited. Closing its Tab never ends it. An ended session keeps its Conversation, read from the
Vault's copy, and Resume starts the Agent again on the same conversation.
_Avoid_: closed, killed, dead

**Workspace**:
herdr's group of Agent sessions, usually one per folder or branch copy. Each Task is one
Workspace named after its title.
_Avoid_: project, group

**Transcript**:
The record an Agent writes of one conversation, held on the Host. An Agent session has
one current Transcript; a Conversation shows only that one, marking each switch. The Vault
of the Agent session's Task keeps its own copy of every Transcript, so the Conversation can
be read when its Host is off and survives the Agent deleting old Transcripts.
_Avoid_: log, history

**Session log**:
The ordered record of everything a Conversation shows for one Agent session, kept on the
machine the Agent runs on and written there once, from its Transcript and its screen. Every
device shows the same Session log; the Vault keeps a copy for search and offline reading.
_Avoid_: event stream, feed

**Conversation**:
The screen that shows one Agent session as chat and takes the user's messages. It is one
face of an Agent session Tab; the other face is the Agent's own terminal, switched by one
shortcut on the Mac and one button on the phone.
_Avoid_: chat view, thread

**Session restore**:
Bringing a mosh Session back after iOS has terminated the app, from state the client
saved on the way to the background. Planned, not in the first build. Never applies to
SSH.
_Avoid_: reconnect (that is a new Session in the same Tab)

**Upload**:
One image or video sent from the phone's library to the Host. It keeps a name carrying the
moment it was sent, and its remote path is inserted into the terminal or the Draft being
written, so the remote program is told where to find it. An image Upload is downscaled
unless the user turns that off, so it need not match the photo in the library; a video
always does. Uploads outlive the Session and the app never deletes them.
_Avoid_: attachment, transfer, image

## Tasks

**Task**:
One piece of the user's work, with its Journal and its Tabs, known by a title in plain
words, such as "Fix the flaky login test". Tasks sit in folders and subfolders, in the
order the user puts them. The Tabs of one Task may run on different Hosts, and some only on the Mac.
Every Task belongs to one Vault. A Task usually has a main Worktree, where its new Tabs
start; a Task without one starts them in the home folder.
_Avoid_: project, ticket, note

**Worktree**:
A git worktree a Task works in, on a Vault Host or on the Mac, usually on its own
branch. A Task has one main Worktree, where its new Tabs start by default, and may have
more, each made for an Agent session that works in parallel on its own branch. The main
one's branch and folder are suggested from the Task's title; no Worktree is ever renamed.
The main one's herdr Workspace carries the title itself, and a session's own Worktree
opens its Tab in that Workspace too.
_Avoid_: branch copy, checkout, project

**Close**:
To finish a Task: its Transcripts are copied into the Vault one last time, its Agents are
stopped, every one of its Worktrees is removed with its branch kept, and the Task is
archived. Its Journal, Documents and Conversations stay readable and searchable, and it can
be reopened. Uncommitted work in a Worktree is given up only when the user says so.
_Avoid_: delete, archive, done

**Vault**:
A store of Tasks, folders and notes, kept on one Host, with a copy on each Mac and phone
that opens it. The user keeps separate Vaults for separate lives, such as work and hobby.
A Vault lists the Hosts its Agent sessions run on, starting with its own; a Host is in at
most one Vault. The Mac is in none, and serves every Vault. The phone opens a Vault by its
name and the Host from its own list that answers to the Mac's ssh alias for it; the phone
cannot reach the Mac, so Tabs on the Mac stay closed there.
_Avoid_: workspace, library, database

**Unfiled**:
The Agent sessions on a Vault's Hosts that no Task has adopted yet, usually because they
were started outside Sesh. The Mac's own unadopted sessions form one more Unfiled list,
shared by every Vault.

**Adopt**:
To make an Unfiled Agent session part of a Task. A session started from a Task is adopted
from birth.
_Avoid_: import, attach, link

**Journal**:
A Task's working log and its first Tab, which never closes or moves. It is a list of
Entries, newest first. The user writes, edits and deletes
every entry; Sesh never adds one on its own.
_Avoid_: log, timeline, history

**Entry**:
One item in a Journal: Markdown text stamped with a moment.
_Avoid_: note, post, record

**Document**:
A Markdown file shown in a Tab of a Task, for reading and editing. It lives either in the
Vault or on a Host, as a file an Agent wrote.
Clicking a Markdown path in a Conversation opens it as a Document in the same Task; any
other file opens read-only. A Task keeps every Document it has opened, so a closed one can
be reopened.
_Avoid_: note, page, file

**Conflict copy**:
The losing text kept when two devices edited the same Entry or Document. It sits beside the
winner until the user deletes it.

**Bookmark**:
A quick way to add an Entry that quotes one item of a Conversation, or a passage of it,
and links to it. The Entry is ordinary afterwards, and the same link works in any Entry or
Document. Following it opens that Agent session at that item, switching to its Task if it
belongs to another.

## Touch

**Touch mode**:
One of three ways the terminal treats fingers: Type, Click or Select. Chosen with a
selector in the Keys row; the app remembers the last one.

**Type mode**:
The default Touch mode. A tap is a mouse click for the remote program and the on-screen
keyboard is up; a drag scrolls; nothing selects text.
_Avoid_: keyboard mode

**Click mode**:
Like Type mode but the on-screen keyboard stays down, for working a TUI by touch.
_Avoid_: tap mode, navigation mode

**Select mode**:
The Touch mode in which a drag selects text to copy, a tap clears the selection, the
keyboard stays down, and the remote program sees no mouse events at all.
_Avoid_: copy mode, selection mode

**Keys row**:
The strip above the keyboard holding modifiers, arrows, symbols, paste, the Editor button
and the Touch mode selector. It stays on screen in every Touch mode.
_Avoid_: toolbar, accessory bar, mini keys

**Modifier**:
A keys-row key that changes the next key or tap: ctrl, alt, shift, cmd and right-click.
One tap arms it for the next input; two taps lock it until tapped again.
_Avoid_: sticky key

**Editor**:
A plain, proportional-font place to compose long input and send it to the active Tab.
It holds a list of Drafts.
_Avoid_: composer, scratchpad, notes

**Draft**:
One saved text in the Editor. Sending a Draft to a Tab leaves it in place.
_Avoid_: note, snippet, prompt
