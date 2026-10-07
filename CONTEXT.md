# Sesh

A terminal for iOS and the Mac that reaches remote machines over SSH or mosh. On the
phone nothing runs locally; on the Mac a Tab may also run on the Mac itself.

## Language

**Host**:
A saved way to reach one remote machine: address, user, transport, key, flags. The
main screen lists Hosts.
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
One thing open at a time in Sesh. Inside a Task a Tab is the Journal, a Markdown note, a
Terminal or an Agent session, and every Tab but the Journal can move to another Task.
Outside any Task, as on the phone today, a Tab shows one Host in one UI mode. Several Tabs
may be open at once; one is active.
_Avoid_: pane, page

**UI mode**:
What a Tab shows for its Host: Terminal or Projects. A Host carries a default UI mode,
and a long press opens it in the other one. A Tab never changes UI mode; switching means
opening another Tab.
_Avoid_: view, app mode

**Terminal**:
The UI mode that shows one Session.

**Projects**:
The UI mode for someone who does not use a terminal: browse and create folders on the
Host and start an Agent session in one.
_Avoid_: file browser

**Agent**:
The coding agent behind an Agent session: Claude, Codex or pi. Other coding agents running
on the Host are not Agents, and Sesh does not show them.
_Avoid_: model, tool, kind

**Agent session**:
One conversation with one Agent, running on the Host in one folder, optionally on its own
branch. It outlives every Session and Tab. Projects lists all of them on the Host,
including those started outside Sesh, and can start one of any Agent. When the Agent
clears or resumes, it moves to another Transcript but stays the same Agent session.
Always "Agent session" in full; a bare Session is the SSH or mosh connection.
On the Mac, every Agent session belongs to one Task once adopted, and keeps belonging after
its Tab closes and its Agent exits. It can move to another Task but never back to Unfiled.
_Avoid_: Claude session, thread, agent pane

**Workspace**:
herdr's group of Agent sessions, usually one per folder or branch copy. At Home, Projects
lists every Workspace under one collapsible heading and nests a repo's branch copies
under its main checkout, as herdr's sidebar does. Those with Agent sessions come first,
the most recently changed leading; the rest follow by name.
_Avoid_: project, group

**Transcript**:
The record an Agent writes of one conversation, held on the Host. An Agent session has
one current Transcript; a Conversation shows only that one, marking each switch. The Vault
of the Agent session's Task keeps its own copy of every Transcript, so the Conversation can
be read when its Host is off and survives the Agent deleting old Transcripts.
_Avoid_: log, history

**Conversation**:
The screen that shows one Agent session as chat and takes the user's messages. On the
phone it opens from Projects, never from a Terminal Tab. On the Mac it is one face of an
Agent session Tab; the other face is the Agent's own terminal, and one shortcut switches
between them.
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
Every Task belongs to one Vault. A Task usually has one Worktree, where its new Tabs
start; a Task without one starts them in the home folder.
_Avoid_: project, ticket, note

**Worktree**:
The git worktree a Task works in, on a Vault Host or on the Mac, usually on its own
branch. Its branch and folder are suggested from the Task's title and never renamed; its
herdr Workspace carries the title itself. A Task has at most one.
_Avoid_: branch copy, checkout, project

**Close**:
To finish a Task: its Transcripts are copied into the Vault one last time, its Agents are
stopped, its Worktree is removed with the branch kept, and the Task is archived. Its
Journal, Documents and Conversations stay readable and searchable, and it can be reopened.
Uncommitted work in the Worktree is given up only when the user says so.
_Avoid_: delete, archive, done

**Vault**:
A store of Tasks, folders and notes, kept on one Host, with a copy on each Mac and phone
that opens it. The user keeps separate Vaults for separate lives, such as work and hobby.
A Vault lists the Hosts its Agent sessions run on, starting with its own; a Host is in at
most one Vault. The Mac is in none, and serves every Vault.
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
