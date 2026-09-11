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
  arrows (hold to repeat) | pinned: paste, Editor, and the three-way Touch mode selector.
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
  scrolls, keyboard down, the program sees no mouse events. A hardware keyboard works in
  every mode. The last mode is remembered.
- Editor: a sheet listing Drafts, first line as title, newest edit first, swipe to
  delete, tap to edit. Editing screen: system proportional font, Send, Send + Enter,
  Cancel. Send uses bracketed paste when the program enabled it; Send + Enter appends
  `\r`. Sending leaves the Draft in place. Drafts persist as JSON.
- Theme: ghostty config `theme = light:Catppuccin Latte,dark:Catppuccin Mocha`. Chrome,
  keys row and Editor use the same palettes, hard-coded from catppuccin's hex values,
  following the system appearance. JetBrains Mono for the terminal and all chrome; the
  Editor's text view is the one system-font exception. Font size is a setting, default
  12pt; pinch-to-zoom changes it for that Tab only.
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

## Follow-ups after phase 5

- Bump libghostty. As of 11 September 2026 the fork's main is 2,753 commits past our
  pinned March commit but has no prebuilt xcframework release for its head, and cmux
  main still pins the same March commit. Bumping means a zig build of the fork; do it
  once cmux moves or a release for a newer commit appears.
- Session restore for mosh (see CONTEXT.md).
