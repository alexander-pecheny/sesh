# Sesh

An iOS terminal that reaches remote machines over SSH or mosh. Nothing runs locally;
every terminal is a window onto a remote shell.

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
The screen that shows one Session. Several Tabs may be open at once; one is active.

**Session restore**:
Bringing a mosh Session back after iOS has terminated the app, from state the client
saved on the way to the background. Planned, not in the first build. Never applies to
SSH.
_Avoid_: reconnect (that is a new Session in the same Tab)

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
The Touch mode in which a drag selects text to copy, the keyboard stays down, and the
remote program sees no mouse events at all.
_Avoid_: copy mode, selection mode

**Keys row**:
The strip above the keyboard holding modifiers, arrows, symbols, paste, the mode toggle,
the Editor button and hide-keyboard.

**Modifier**:
A keys-row key that changes the next key or tap: ctrl, alt, shift, cmd and right-click.
One tap arms it for the next input; two taps lock it until tapped again.
_Avoid_: sticky key
_Avoid_: toolbar, accessory bar, mini keys

**Editor**:
A plain, proportional-font place to compose long input and send it to the active Tab.
It holds a list of Drafts.
_Avoid_: composer, scratchpad, notes

**Draft**:
One saved text in the Editor. Sending a Draft to a Tab leaves it in place.
_Avoid_: note, snippet, prompt
