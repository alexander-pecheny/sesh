# Sesh implementation plan

Sesh is a native iOS terminal for SSH and mosh, rendered by libghostty. Read
`CONTEXT.md` for the vocabulary and `docs/adr/` for the decisions with reasons. This file
is the spec for the implementing agents. Decisions here were settled with the owner on
11 September 2026; do not reopen them, ask if something is missing.

## Facts about the machine

- Xcode 26.3, iOS 26.2 simulator, an iPhone 17 Pro simulator is booted (udid
  `466D3ACF-302A-40B8-8570-719088AF057C`). Drive it headlessly: `xcodebuild`,
  `xcrun simctl install/launch`, `axe`. Never `open -a Simulator`.
- Signing: Apple Development certificate "ap@pecheny.me (9JTT8XLTCV)" whose team is
  `B5T934YFU5`; a managed wildcard profile for that team covers the paired iPhone 17 Pro.
  Xcode has no signed-in account, so `just device` signs the bundle by hand. Bundle id
  `me.pecheny.sesh`.
- Rust 1.96 with targets `aarch64-apple-ios` and `aarch64-apple-ios-sim` installed.
  `cbindgen` is not installed: `cargo install cbindgen`.
- `xcodegen`, `just`, `zig 0.15.2`, `mosh-server` (Homebrew), `protoc` available.
- JetBrains Mono 2.304 is in `~/Library/Fonts/JetBrainsMono-2.304` (OFL licence; copy
  the TTFs and the licence into the bundle).
- rmosh: `~/rmosh`, remote `git@code.pecheny.me:pecheny/rmosh.git`. Pure Rust, pure-Rust
  crypto, `#![forbid(unsafe_code)]` outside `mosh-sys`. Read its `AGENTS.md`,
  `CONTEXT.md` and `docs/adr/` before touching it.
- libghostty: the cmux fork `github.com/manaflow-ai/ghostty`. A local checkout is at
  `~/cmux/ghostty` (commit `bc9be90a21997a4e5f06bf15ae2ec0f937c2dc42`). Prebuilt
  `GhosttyKit.xcframework` with `ios-arm64`, `ios-arm64-simulator` and macOS slices is
  published per commit at
  `https://github.com/manaflow-ai/ghostty/releases/download/xcframework-<sha>/GhosttyKit.xcframework.tar.gz`;
  see `~/cmux/scripts/download-prebuilt-ghosttykit.sh` and
  `~/cmux/scripts/ghosttykit-checksums.txt` for the pattern and the checksum for that
  commit. A copy of the built framework is in `~/.cache/cmux/ghosttykit/<sha>/`.
- Phase 1 findings on the embedded API live in `docs/ghostty-embedding.md`. Read it.
- Manual I/O in the fork: `ghostty_surface_config_s.io_mode = GHOSTTY_SURFACE_IO_MANUAL`
  with `io_write_cb`/`io_write_userdata` receiving what the terminal wants sent to the
  remote, and `ghostty_surface_process_output(surface, bytes, len)` pushing remote
  output into the surface. See `~/cmux/ghostty/include/ghostty.h`,
  `src/termio/Manual.zig` and `src/apprt/embedded.zig`.
  Ghostty's Swift wrappers in `~/cmux/ghostty/macos/Sources/Ghostty/` (MIT) are
  reference material; the iOS `SurfaceView_UIKit.swift` is 132 lines with no input
  handling, so the terminal view is ours.
- Forgejo: API token in `~/.config/forgejo/token`, host `code.pecheny.me`, git push over
  ssh. Create the repo `pecheny/sesh` there in phase 1.
- Local test target: the Mac's sshd is off. Run an unprivileged `/usr/sbin/sshd -D -f
  <config>` on port 2222 with a generated host key, an `AuthorizedKeysFile` pointing at a
  test key, and `PidFile`/`HostKey` under the repo's ignored `.local/`. The simulator
  reaches it at `localhost:2222`. `mosh-server` from Homebrew serves mosh.

## Decisions

- Transports: SSH via `russh` and mosh via rmosh, both inside one Rust crate
  `core/sesh-core` exposing a C ABI, built into `SeshCore.xcframework` (ADR 0002).
- libghostty from the cmux fork for manual I/O (ADR 0001). Built from source by
  `just ghosttykit` with the libxev patch; the fork's prebuilt frameworks hang on iOS.
- rmosh is a git submodule at `vendor/rmosh`. The client session loop moves from
  `crates/mosh-client/src/main.rs` into the library, taking an input byte source, an
  output byte sink and a size signal instead of a tty fd. The binary stays as a thin
  caller. rmosh's tests stay green; push that change to rmosh's own remote on a branch
  and point the submodule at it.
- Host: name (default `user@address`), address, port, user, Key, Transport (one of
  ssh/mosh, fixed), agent forwarding toggle (SSH only), Extra flags for ssh, Extra flags
  for mosh, optional Remote command. Persisted as JSON in Application Support.
- Keys: imported by paste or file picker, optional passphrase, stored in the Keychain,
  public key shown with a copy button. Several Keys, one chosen per Host. Also password
  and keyboard-interactive auth, prompted in a sheet; password may be saved to the
  Keychain per Host. Agent forwarding means the app answers
  `auth-agent@openssh.com` channel requests with its own Keys.
- Host key verification: trust on first use with a fingerprint sheet. Known hosts stored
  by the app, keyed by address and port. On mismatch refuse, show both fingerprints,
  offer replace.
- Extra flags subset. ssh: `-p`, `-l`, `-4`, `-6`, `-J user@host[:port]` (one hop),
  `-t`, `-A`, `-o` with `ServerAliveInterval`, `ServerAliveCountMax`, `Port`, `User`,
  `HostKeyAlgorithms`, `PubkeyAcceptedAlgorithms`. mosh: `--ssh=` (same ssh subset),
  `-p`/`--port`, `--server=`, `--predict=`, `-a`, `-n`, `--no-init`, `-4`, `-6`,
  `--experimental-remote-ip=local|remote` with `remote` the default. Anything else is
  rejected before connecting, naming the flag.
- The Remote command runs as `exec "$SHELL" -lic '<command>'` on both transports, so
  it sees the PATH and aliases the user's rc files set; a bare `execvp` did not.
- Mosh bootstrap: run `mosh-server new -s -c 256 -l LANG=... [-p port] [-- command]`
  over a russh exec channel with a pty, prefixed by the `remote` announce when chosen,
  parse `MOSH CONNECT <port> <key>` as rmosh's launcher does (`crates/mosh/src/main.rs`,
  reuse its parsing by moving it into a library function), then start the rmosh client
  in-process over UDP. The SSH session ends after the handshake.
- Tabs: many, any Host any number of times. Tab bar on top with the active Host name and
  a switcher. Closing the last Tab shows the Host list. Backgrounding: mosh resumes by
  itself on foreground; an SSH Tab shows a disconnected overlay with a reconnect button
  that starts a new Session in the same Tab. No keepalive tricks, no retry loops.
  Session restore across app termination is phase two of the product, out of scope now.
- Keys row, left to right: `esc ctrl alt shift cmd right-click tab` | `~ | / -` |
  arrows left up down right, then home pgup pgdn end | pinned: paste, Editor, and the three-way Touch mode selector.
  No separate hide-keyboard button: Click and Select modes put the keyboard down.
  Modifiers: one tap arms for the next key or tap, two taps lock, tap again to clear;
  highlighted while armed or locked. `cmd` is ghostty's super. The row scrolls sideways
  and also shows with a hardware keyboard.
- Three Touch modes, picked with a segmented icon selector pinned at the right of the
  Keys row. Type (default): tap sends mouse press and release at the cell (right button
  when right-click is armed), the on-screen keyboard is up; one-finger drag scrolls, as
  scrollback or wheel events when the program enabled mouse reporting; no selection;
  long press does nothing. Click: same taps and drags, but the on-screen keyboard stays
  down so the user can work a TUI by touch; the Keys row stays visible. Select:
  one-finger drag selects with a floating Copy button, tap clears, two-finger drag
  scrolls, keyboard down, the program sees no mouse events and a tap only clears the
  selection. A hardware keyboard works in every mode. The last mode is remembered.
- The Keys row is a normal view under the terminal, not the keyboard's accessory view:
  as an accessory it overlapped the last row, sat in the home-indicator gesture zone
  when the keyboard was down, and made the grid flap on a real phone.
- Editor: a sheet listing Drafts, first line as title, newest edit first, swipe to
  delete, tap to edit. Editing screen: system proportional font, Send, Send + Enter,
  Cancel. Send uses bracketed paste when the program enabled it; Send + Enter appends
  `\r`. Sending leaves the Draft in place. Drafts persist as JSON.
- Theme: ghostty config `theme = light:Catppuccin Latte,dark:Catppuccin Mocha`. Chrome,
  keys row and Editor use the same palettes, hard-coded from catppuccin's hex values,
  following the system appearance. JetBrainsMono Nerd Font for the terminal; the chrome
  uses the system font (changed 12 September 2026, it was mono before). Font size is a setting, default
  12pt; pinch-to-zoom changes it for that Tab only.
- Clipboard: OSC 52 writes set the phone's clipboard (`clipboard-write = allow`);
  reads are denied.
- Platform: iOS 18 minimum, universal iPhone and iPad. SwiftUI for lists, forms, Editor,
  Tab bar. A UIKit `UIView` we own for the terminal: `CAMetalLayer`, `UIKeyInput` (and
  `UITextInput` only if the keyboard needs it), hardware key events via
  `pressesBegan`, gesture recognisers for the two touch modes.
- Project: XcodeGen `project.yml`, never hand-edit the pbxproj. `justfile` targets:
  `ghosttykit` (download and verify), `core` (cargo build both targets, cbindgen,
  `xcodebuild -create-xcframework`), `gen`, `build`, `run` (install and launch on the
  booted simulator), `sshd` (start the local test sshd), `e2e`. Both xcframeworks are
  gitignored.

## Code rules

- Follow `~/CLAUDE.md`: at most one comment line per hundred lines, only for a why; ask
  how each piece could be shorter; python only through `uv`; never bring the simulator
  window forward; drive it with `axe`.
- Commits: small, imperative subject, body only when the why is not obvious, and the
  attribution trailer given in your instructions. Push to Forgejo at the end of each
  phase.
- Swift: no third-party Swift packages. Rust: keep `sesh-core` free of `unsafe` outside
  the FFI module. The C ABI is the only boundary between Swift and Rust; no callbacks
  into Swift except the byte-output and event callbacks, all documented in the header.

## C ABI sketch for `sesh-core`

```
sesh_session_t* sesh_ssh_connect(const sesh_ssh_config_t*, sesh_callbacks_t, void* ud);
sesh_session_t* sesh_mosh_connect(const sesh_mosh_config_t*, sesh_callbacks_t, void* ud);
void sesh_session_write(sesh_session_t*, const uint8_t*, size_t);   // keystrokes to remote
void sesh_session_resize(sesh_session_t*, uint16_t cols, uint16_t rows);
void sesh_session_close(sesh_session_t*);
void sesh_session_free(sesh_session_t*);
// callbacks: on_output(ud, bytes, len), on_state(ud, state, message),
// on_host_key(ud, fingerprint, known_status) -> answer via sesh_session_answer_host_key,
// on_auth_prompt(ud, prompt, echo) -> answer via sesh_session_answer_prompt.
uint32_t sesh_parse_flags(transport, const char* text, char** error);  // validation only
```

Keys live in Swift's Keychain and are passed as PEM bytes at connect time. Known hosts
are checked in Rust against a file path the app passes in. Exact shapes are the phase 2
agent's call; keep the surface this small.

## Phases

Each phase is one agent, run in order. Each ends with `just build` green, the app
launched on the simulator with a screenshot proving the acceptance line, and a pushed
commit. Do not start the next phase's work.

1. Scaffold. `git init`, `.gitignore`, `README.md` (two paragraphs), `justfile`,
   `project.yml`, the GhosttyKit download script with checksum, fonts and theme,
   the terminal `UIView`, a `Ghostty.App`-style wrapper. Acceptance: a Tab showing a
   ghostty surface in manual I/O mode wired to a loopback that echoes typed bytes, with
   JetBrains Mono and Catppuccin Mocha visible, software keyboard typing into it.
   Create the Forgejo repo `pecheny/sesh` and push.
2. SSH. `core/sesh-core` with russh, the C ABI, `SeshCore.xcframework` build, Keys
   screen, Host list and form, known-hosts sheet, auth prompt sheet, SSH Tab. Local
   sshd script. Acceptance: connect to `localhost:2222` with an imported test key, run
   `ls`, see the trust-on-first-use sheet once and never again.
3. Mosh. rmosh client refactor on a branch in `~/rmosh` (push it), submodule at
   `vendor/rmosh`, bootstrap and client in `sesh-core`, mosh Tab. Acceptance: a mosh
   Session to localhost survives backgrounding the app for a minute and typing resumes.
4. Input. Keys row, modifiers including right-click, arrows with repeat, paste, click
   and select modes, Editor with Drafts, font size setting and pinch zoom. Acceptance:
   ctrl-c interrupts `sleep 100`; a tmux pane click focuses the pane in click mode;
   select mode copies a word; a two-line Draft arrives in `cat` as one paste.
5. Finish. Tab bar and switcher, disconnected overlay and reconnect, agent forwarding,
   password save, `just e2e` running the phase 2 to 4 acceptance checks with `axe`,
   README updated. Acceptance: `just e2e` passes on the booted simulator.

## Phase 6: Uploads

Settled with the owner on 16 September 2026. An Upload is one image or video sent from the
phone's library to the Host; see `CONTEXT.md`.

**Transport.** SFTP, via `russh-sftp` 3.0 over a channel on the live russh handle. An SSH
Session already holds one, so an Upload costs a channel. A mosh Session dropped its handle
after bootstrapping (`mosh.rs`), so it opens a fresh SSH connection per Upload and closes
it after: mosh roams, and a cached socket would be stale exactly when it is wanted. The
host key is already in `known_hosts`, so no sheet unless it genuinely changed; a Host with
password auth and no saved password re-prompts through the existing `AuthSheet`, whose
"Save password" toggle stops the asking.

Not scp: OpenSSH 9.0 deprecated the protocol, and iOS cannot shell out to an `scp` binary
anyway. Not `exec cat >`: SFTP gives typed status codes, so "permission denied" and "no
space left on device" reach the user as themselves.

**Destination.** `~/.sesh/uploads`, created on demand, resolved once per Session with
SFTP `realpath` so the inserted path is absolute. A tilde only expands when a shell reads
the word, and the path may be read by a program instead. Nothing prunes the directory;
deleting a user's remote files on a timer they never set is worse than a directory that
grows.

**Names.** `20260916-143022-IMG_4821.jpg`: the date, the time, then the picker's suggested
name. Sorts by date in the directory. A multi-select batch lands in the same second, so
Rust `stat`s and suffixes on collision.

**Bytes.** Swift decides the name and the format, Rust moves the bytes.

- Compress on, the default: any image to at most 1568px on the longest edge, JPEG q0.8.
  1568px is where Claude's vision tops out, so more buys nothing for the main use.
- Compress off: HEIC becomes JPEG, everything else goes up untouched. That transcode is
  about the Host being able to open the file, not about size.
- Video: never touched, never compressed. Installing ffmpeg is the Host's problem.
- `Store.compressUploads`, persisted like `fontSize`, one toggle in Settings.

**Picker.** `PHPickerViewController`, `selectionLimit = 0`, filtered to images and videos.
It runs out of process, so there is no `NSPhotoLibraryUsageDescription` and no permission
prompt. Swift prepares each pick into the app's temp directory and passes *paths* to the
core, never bytes, so a 200MB video never sits in memory.

**Two buttons.** The Editor is a full-height sheet, so the Keys row is off screen while a
Draft is open and one button cannot serve both. The Keys row gets one after `paste`, icon
`image-up` (a new lucide imageset), accessibility label "upload"; the Draft toolbar gets
one beside Send. Whichever button was tapped is the insertion target, so no code has to
work out what has focus.

**Insertion.** Paths joined by a single space. Never a newline: `terminal.paste` calls
`ghostty_surface_text`, which is plain text input rather than a bracketed paste, so a
newline in a Draft submits the line. Each path is shell-quoted only when it holds a
character outside `A-Za-z0-9._/-`, which our names never do, so quoting fires only for an
odd home directory. Prepend a space unless the character before the caret is whitespace or
the field is empty; always append one. In a Draft, insert at the caret and replace any
selection, using `TextEditor(text:selection:)`; append at the end if it was never focused.

**While it runs.** The button becomes a determinate ring measuring bytes across the whole
batch, and tapping it cancels. It is disabled unless the Session is connected, and while
an Upload is in flight: the ring is the button, so it cannot also start something new.
Tabs hold their own Sessions, so one Tab uploading never blocks another. Paths appear only
after the whole batch lands, so a path in your text always names a file that exists.

**Failure.** Any error, or a cancel, unlinks whatever already landed and inserts nothing,
so a Draft never quietly under-describes itself. cmux does the same
(`~/cmux/Sources/TerminalImageTransfer.swift`, `Workspace.swift:4434`). The SFTP message
is shown in an `.alert` bound to one `@Published var uploadError: String?`, attached both
in `SessionTab` and in the Draft, because an alert under a sheet will not appear over it.
A cancel shows nothing; the user knows they cancelled.

**Shape.** New `core/sesh-core/src/upload.rs`. Two `Command` variants carry a request in,
two `Events` methods carry progress and the finished paths out. `ssh.rs` wraps its handle
in an `Arc` — `Handle` is not `Clone` but `channel_open_session` takes `&self` — and
spawns the task so the terminal keeps pumping; `mosh.rs` spawns a task that connects
fresh. The C ABI gains:

```c
uint32_t sesh_session_upload(sesh_session_t *, const sesh_upload_t *files, size_t count);
void sesh_session_cancel_upload(sesh_session_t *, uint32_t id);
// in sesh_callbacks_t:
void (*on_upload_progress)(void *userdata, uint32_t id, uint64_t done, uint64_t total);
void (*on_upload_done)(void *userdata, uint32_t id, const char *const *paths, size_t count,
                       const char *error);
// sesh_upload_t { const char *local_path; const char *remote_name; }
```

Acceptance: pick three photos in the terminal, see the ring fill, see three space-separated
absolute paths land at the cursor, and `ls -l` them on the Host. Repeat from a Draft. Kill
the link mid-batch and confirm the uploads directory is empty.

## Phase 7: Projects

Settled with the owner on 27 September 2026. Projects is a UI mode for someone who does
not use a terminal; see `CONTEXT.md` and ADR 0004.

**UI mode.** A Tab is a Host plus a UI mode, Terminal or Projects. The Host form gains a
default UI mode; tapping the Host opens it, a long press offers the other. A Tab never
switches UI mode.

**Connection.** A Projects Tab holds one SSH connection, a Link, even for a mosh Host.
Everything, listing included, runs over exec channels through `"$SHELL" -lic`, as the
Remote command does: one `sesh_session_run` call is a smaller C ABI than SFTP directory
calls, and `mkdir`'s stderr already names its failure. On foreground the Tab
reconnects silently and refreshes, with no disconnected overlay. While visible it polls
`herdr agent list` every 3 seconds.

**Tools.** On connect, check `command -v herdr claude git`. A missing `herdr` or
`claude` shows a screen naming it with its install command and a copy button; a missing
`git` only hides the branch switch. Sesh never installs anything. If `herdr status`
reports no server, start `herdr server` detached. Everything runs in the default herdr
session of the Host's user.

**Picker.** Starts at home and cannot go above it. Folders only, no dot-entries. "New
folder" in every folder; names may not contain `/` or start with `.`. At the top of home,
a section lists every `claude` agent in herdr with its state, filed under its
workspace's folder; one on a branch carries the branch name as a subtitle.

**Start.** A sheet with a name pre-filled from the folder and two words, editable,
normalised to `[a-z][a-z0-9_-]{0,31}` and unique among live agents; and, only when the
folder holds `.git`, an "On a new branch" switch whose branch takes the same name.
Pressing Start finds or creates the folder's herdr workspace (or runs `herdr worktree
create --cwd <folder> --branch <name>`), opens a pane, and runs
`herdr agent start <name> --kind claude --pane <id> -- --remote-control <name>
--dangerously-skip-permissions`. Permissions are always bypassed; there is no switch.

**After.** Read the pane with `herdr agent read` and pull out the `https://claude.ai/code`
link. Each Claude session row gets "Open in Claude" following it, or "Find it in the
Claude app by name" if none was found. Claude asks once per untrusted folder tree whether to
trust it; Sesh answers yes, since picking the folder and pressing Start is her answer. If
start still fails or reports `blocked`, show the pane's last lines; a blocked first run means the bypass confirmation, so name
`skipDangerousModePermissionPrompt` in `~/.claude/settings.json`. A swipe offers Stop:
ctrl-c twice via `herdr agent send-keys`, then close the pane. Branches and their copies
stay on disk.

Acceptance: on a Host whose user has herdr and a signed-in claude, create a folder, start
a Claude session in it and another on a new branch in a repo, open one in the Claude app
from its link, see its state change in Projects, and stop it.

## Phase 8: Conversations

Settled with the owner on 5 October 2026, building on a September session that was never
written down. See `CONTEXT.md` (Agent, Agent session, Transcript, Conversation) and ADR
0005. Two agents in parallel: one in the herdr fork (`~/herdr`), one in Sesh. The
protocol below is the contract between them.

### herdr fork

`follow`, `history`, `entry`, `answer` and `permit` have since moved into Sesh's own
`sesh-transcript` with the same protocol (Phase 9); the fork keeps its copies for now and
only adds Transcript paths and permission hooks.

**Transcript paths.** `session_ref_from_report` keeps `agent_session_path` for every
Agent, not only pi and omp. The Codex hook sends `transcript_path` as
`agent_session_path`. `agent list` reports both id and path. Resume keeps working.

**Permission hooks.** `herdr integration install claude|codex` also installs a
`PermissionRequest` hook. It reports the tool, its input and any reason to herdr over the
socket and returns at once with no decision, so the Agent draws its usual prompt.

**`herdr agent follow <pane> [--since <cursor>] [--last N]`.** Long-running. Writes one
JSON object per line, flushed per line, until killed. N defaults to 50 entries.
`herdr agent follow --protocol` prints the protocol number, `2`, and exits.

```
{"t":"hello","protocol":2,"agent":"claude|codex|pi","transcript":"<path>"}
{"t":"entry", ...entry}
{"t":"state","state":"idle|working|blocked|done"}
{"t":"switch","reason":"clear|resume|new|compact|fork|other","transcript":"<path>"}
{"t":"permission","id":"<id>","tool":"<raw name>","summary":"<one line>","command":"<shell, if any>","file":"<path, if any>","reason":"<text, if any>"}
{"t":"permission_done","id":"<id>"}
{"t":"cursor","cursor":"<opaque>"}
```

Every entry has `id` (stable across reconnects, opaque to Sesh), `kind`, `summary` (one
plain line, used for kinds Sesh does not know) and `at` (RFC 3339). A `tool` entry's id
comes from the Agent's own call id (`<tag>.call.<call id>`), and its `result` computes
`call` the same way from the result line alone, so a result names its call even when
the call was never sent; it can arrive before the call does. Other ids come from the
line's byte offset (`<tag>.<offset>.<n>`), a question's from its questions. Kinds:

- `user`: `text`, `images` (remote paths).
- `text`: `text`, markdown.
- `thinking`: `text`, `seconds` if known.
- `tool`: `tool` (one of `edit write bash read search fetch task other`), `name` (raw),
  `file`, `command`, `description` as they apply.
- `result`: `call` (the `tool` entry's id), `text`, `diff` (unified, for edit and write),
  `added`, `removed`, `error` (bool), `truncated` (bool), `answers` (for a question, one
  per question).
- `todo`: `items`, each `text` and `status` (`pending in_progress completed`).
- `question`: Claude's `AskUserQuestion`: `questions`, each `question`, `header`,
  `multi`, `options` (each `label`, `description`).

`result.text` and `diff` are cut to the first and last 40 lines and 16 KB; `truncated`
says so. `herdr agent entry <pane> <id>` prints that entry whole, as one JSON line.

`follow` reads only the end of the Transcript: a window of the last K bytes, aligned to
a line, that doubles until it holds N entries or reaches the file's start. A result
whose call lies before the window still names it.

**`herdr agent history <pane> --before <id> [--last N]`.** N defaults to 50. Prints up to
N entries that precede entry `<id>` in the pane's current Transcript, oldest first,
each as the `{"t":"entry",...}` line `follow` prints, then one last line
`{"t":"history","more":true|false}`; `more` says earlier entries exist. Sesh passes the
id of the oldest entry it has, of any kind. An unknown id exits 1 with a message.

```
{"t":"entry","id":"3f9c2a1b.20871652.0","kind":"user",...}
{"t":"entry","id":"3f9c2a1b.call.toolu_01Hx…","kind":"tool",...}
{"t":"entry","id":"3f9c2a1b.20874410.0","kind":"result","call":"3f9c2a1b.call.toolu_01Hx…",...}
{"t":"history","more":true}
```

A `cursor` follows each burst of lines; after `--since` nothing is repeated or missed.
A switch to another Transcript emits `switch`, then that Transcript's last N entries.
`permission_done` follows when the prompt is answered anywhere or the Agent moves on.
Lines Sesh does not need (system, meta, usage, snapshots) never leave herdr. pi's
Transcript is a tree; follow its current branch, as far back as the window reaches.

**Answering.** `herdr agent answer <pane> --json '<answers>'` plays Claude's question menu
(a digit per option, "Type something" plus text plus Enter, then Enter on Submit).
`herdr agent permit <pane> allow|deny` presses the Agent's key. Both read the screen
afterwards and exit non-zero with the screen text if the menu is still open.

### Sesh

**Core.** `sesh_session_run` gains a streaming form: a callback per chunk of stdout and a
handle to cancel it. Sesh splits lines; the core knows nothing about transcripts.

**Projects.** On connect, `herdr agent follow --protocol` must print a number Sesh knows,
else a screen says "This Host needs Sesh's herdr" with the install steps and Projects
does nothing else. It lists Agent sessions whose Agent is claude, codex or pi and hides
the rest. The Start sheet gains an Agent picker, Claude by default, remembering the last
one; Codex starts with `--dangerously-bypass-approvals-and-sandbox`, pi with no flag.
`command -v` checks the picked Agent instead of `claude`. After Start, the Conversation
opens with the input focused. Tapping an Agent session opens its Conversation; "Open in
Claude" moves into the Conversation's toolbar and shows for Claude only.

**Conversation.** A screen pushed from Projects that runs `follow` and resumes with
`--since` after a reconnect. User messages are right-aligned bubbles with images; agent
text is markdown without a bubble; thinking is one collapsed line; edit and write cards
show the file and +/− counts and expand to the diff; bash cards show the command and
expand to the output; consecutive read, search and fetch calls group into one line; the
newest todo list is pinned above the input; a task is one card with its description;
a question is a card with option lists, a free-text field and Send, calling
`agent answer`; a permission is a card with the command and Allow and Deny, calling
`agent permit`, gone on `permission_done`. Unknown kinds show `summary`. Expanding a
truncated card runs `agent entry` once. A switch draws a divider. The input box sends
through `herdr agent prompt`, has a photo button reusing Upload, and turns Send into Stop
(Esc via `agent send-keys`) while the state is working. No slash-command menu.

Acceptance: on localhost with the fork installed, start one Agent session of each Agent
from Projects, exchange a message with each, see an edit card's diff, answer a Claude
question and a permission prompt from the phone, and reconnect mid-reply without a gap
or repeat.

## Phase 9: transcript helper

Settled with the owner on 6 October 2026; see ADR 0006. Projects now works on any Host with
stock herdr.

**Helper.** `core/sesh-transcript` holds the Transcript parsers moved from the fork, and a
binary that speaks protocol 2 exactly as Phase 8 describes: `follow <pane> [--since C]
[--last N]`, `follow --protocol`, `history`, `entry`, `answer`, `permit`, plus `--version`,
which prints the crate version and a hash of its sources. It reads each pane with
`herdr pane get` every 250 ms, finds the Transcript from the reported path (the fork's
`path`, or stock herdr's `kind: path` for pi), else by session id under
`$CLAUDE_CONFIG_DIR`/`~/.claude/projects` or `$CODEX_HOME`/`~/.codex/sessions`, and presses
keys with `herdr pane send-keys`/`send-text`, reading the screen with `herdr pane read`.
Permission cards appear only on the fork, whose hooks put `permission` on the pane. Its only
dependencies are serde and serde_json.

**Builds.** `just helpers` builds static musl binaries for Linux x86-64 and arm64 (linked
by `rust-lld`, no C toolchain) and macOS arm64 and x86-64 into `build/helpers`, named
`sesh-transcript-<uname -sm, lower-cased, space as dash>.gz`, with the version in
`build/helpers/version`. `just helpers-release` publishes them as the GitHub release
`helper-<version>` (the tag goes to Forgejo first, since its push mirror prunes tags GitHub
alone has) and writes `Resources/helpers.json` from the published files: version, release
URL, SHA-256 by platform. The app carries only that JSON. Run it whenever the helper changes.

**Install.** On connect Projects runs `uname -sm` and `~/.sesh/bin/sesh-transcript
--version`; if the version differs from the pinned one, the Host downloads its build with
curl or wget, checks the SHA-256 and unpacks it. If that fails, the phone downloads the same
file, checks it and puts it there over SFTP (`sesh_session_put`). Every former
`herdr agent follow|history|entry|answer|permit` call runs the helper instead. Projects
refuses only when herdr is missing or the platform has no published helper (anything but
Linux x86-64/arm64 and macOS).

## Phase 10: Tasks on the Mac

Settled with the owner on 7 October 2026; see ADRs 0008 and 0009 and the Tasks section of
CONTEXT.md. A Mac app that joins a notetaker to herdr: each Task has a plain title, a
Journal, an optional Worktree and Tabs, and owns its Agent sessions for good. It is for the
owner alone for now: unsandboxed, built and installed from Xcode. The phone comes later.

**Build order.**

1. A macOS target in this project sharing the Rust core, Hosts and Keys, the Conversation
   model and view. Mac versions only of the shell and of the UIKit-only views (the text
   input, the image viewer). The iOS target does not change.
2. One Vault on vps-he: a SQLite database and Transcript copies beside it, written only by
   the helper, which serves it over the SSH connection. Each device keeps a full copy
   without Transcripts and queues its writes, offline included, from the first build.
   Entries and Documents merge one by one; the later edit wins and the other becomes a
   Conflict copy.
3. The left pane: Vaults, folders and Tasks by title, reordered by dragging.
4. New Task: a title, then an optional Worktree on a Vault Host or the Mac, with branch and
   folder suggested from the title (`pecheny/fix-flaky-login-test`). Sesh installs the
   herdr fork into `~/.sesh/bin` where it is missing, checked against a pinned SHA-256,
   and uses the everyday herdr server; a private one only when a stock or older herdr holds
   it. Each Task is one herdr Workspace named after its title; renaming a Task renames the
   Workspace, never the branch or folder.
5. Tabs: the Journal (Entries newest first, each editable and deletable), Documents from the
   Vault or a Host, Terminals (a plain shell in the Worktree that ends with its Tab) and
   Agent sessions with two faces, the Conversation and the Agent's own terminal, switched by
   one shortcut. Every Tab but the Journal can move to another Task.
6. Transcript copies into the Vault as sessions run; Unfiled per Vault Host plus one for the
   Mac; adopting a session and moving it between Tasks. Adopting prefills the title from
   the Agent's first message.
7. Bookmarks: an Entry quoting one Conversation item, or a passage of it, with a
   `sesh://` link naming the Vault, Agent session and item. Links work in any Entry or
   Document and switch Task when needed. Paths in a Conversation are links: Markdown opens
   as a Document in the same Task, other files read-only, an open file focuses its Tab.
8. Search across every open Vault, run on each Vault Host with SQLite full-text search over
   the database and the Transcript copies; offline, the device searches its copy without
   Conversations and says so. A hit opens its exact place.

**Reaching machines.** The Mac app runs everything through `/usr/bin/ssh` with the owner's
`~/.ssh/config`, one ControlMaster per Host, so a Host on the Mac is an ssh alias such as
`vps-he`, and the Mac itself is run through `/bin/zsh -lc`. The iOS app's Hosts and Keys
stay on the phone for now. Both kinds of machine get the same helper in `~/.sesh/bin`.

**Vault protocol.** The helper serves a Vault from a folder, `~/.sesh/vaults/<name>/`,
holding `vault.db` (SQLite, WAL, FTS5) and `transcripts/<session>/<file>`. Every row is a
record: `{"id", "kind", "body", "seq", "deleted"}`, where `id` is a UUID the client makes,
`kind` is `folder`, `task`, `entry`, `document`, `session`, `tab` or `conflict`, `body` is a
JSON object the helper stores as it came, and `seq` is the Vault's change counter at the
record's last write. The helper reads only `body.title`, `body.text`, `body.task` and
`body.edited` (milliseconds since 1970). Commands, each printing JSON lines:

- `vault init DIR` creates the folder and database if missing and prints `{"t":"head","seq"}`.
- `vault pull DIR --since SEQ` prints every record with a larger `seq` as
  `{"t":"record",...}`, deleted ones included, then `{"t":"head","seq"}`.
- `vault follow DIR --since SEQ` does the same, then keeps printing records as they change.
- `vault push DIR FILE` applies the changes in FILE, one JSON object per line:
  `{"id","kind","body","base","deleted"}`, where `base` is the `seq` the client last saw for
  the record, 0 for a new one. For an `entry` or `document` whose stored `seq` differs from
  `base`, the body with the later `edited` wins and the other is kept as a new `conflict`
  record whose body is `{"of": id, "kind", "task", "title", "text", "edited"}`. Every other
  change simply wins. It prints `{"t":"record",...}` for each record it wrote, then the head,
  and deletes FILE. Clients send changes as a file over SFTP, since a Document may exceed
  the 128 KB a single command-line argument allows.
- `vault size DIR SESSION FILE` prints `{"size"}` of a Transcript copy, 0 when absent.
- `vault append DIR SESSION FILE --offset N BYTES_FILE` appends the bytes when the copy is
  exactly N long, else fails with its real size; `vault copy DIR SESSION --from PATH` does
  the same from a Transcript on this machine. Both index the new whole lines for search.
- `vault search DIR QUERY [--limit N]` prints hits, best first:
  `{"t":"hit","kind","id","task","session","item","snippet"}`. Records match on title and
  text; Transcript copies match on the user's messages and the Agent's replies, with `item`
  the Conversation entry id.
- `follow`, `history` and `entry` take `--file PATH --agent AGENT` in place of a pane, to
  show a Transcript copy when its Agent session's machine is off.

Entry ids hash the Transcript's file name, not its whole path, so a copy and its source give
the same ids and a Bookmark (`sesh://VAULT/SESSION/ITEM`) works on both.

**Later.** State marks on Tasks and a Waiting list, ⌘K to jump by name, a better view of
Conflict copies, archiving with Worktree removal, several Vaults in the UI, the fork on a
public mirror with release builds, distribution, and Tasks on the phone.

## Follow-ups after phase 5

- Bump libghostty. As of 11 September 2026 the fork's main is 2,753 commits past our
  pinned March commit but has no prebuilt xcframework release for its head, and cmux
  main still pins the same March commit. Bumping means a zig build of the fork; do it
  once cmux moves or a release for a newer commit appears.
- Session restore for mosh (see CONTEXT.md).
