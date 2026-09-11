# Embedding libghostty on iOS: what phase 1 learned

- Surface: `ghostty_surface_config_new()`, `platform_tag = GHOSTTY_PLATFORM_IOS`,
  `platform.ios.uiview` = unretained view pointer, `io_mode = GHOSTTY_SURFACE_IO_MANUAL`,
  `io_write_cb` + `io_write_userdata`. `ghostty_surface_new` returns non-null even when
  broken; it is not a health check.
- The Metal layer is ours to size. libghostty adds an `IOSurfaceLayer` sublayer to the
  view's layer, sets its `contentsScale` once at creation and never again, and reads
  that layer's bounds × scale as the drawable size. If that disagrees with
  `ghostty_surface_set_size` it draws nothing. `layoutSubviews` must set
  `sublayer.frame = layer.bounds` and `sublayer.contentsScale` alongside
  `set_content_scale` and `set_size`. No `layerClass` override.
- Threads: `io_write_cb` fires on ghostty's I/O thread; hop to main (or hand straight to
  the Rust session) and never call back into the surface synchronously from it.
  `ghostty_surface_process_output` is safe from any thread and runs the VT parser on the
  caller. Call `_key`, `_text`, `_set_size`, `_set_focus`, `_update_config` on main.
- Grid size: `ghostty_surface_size` returns columns and rows once `ghostty_surface_set_size`
  has run, which is how a transport learns the window size to send the remote.
- Actions matter: answer `GHOSTTY_ACTION_RELOAD_CONFIG` by loading a fresh config and
  calling `ghostty_app_update_config` / `ghostty_surface_update_config`, or the
  light/dark theme never switches. Later phases need `SET_TITLE`, `RING_BELL`,
  `CLOSE_WINDOW`, `SHOW_ON_SCREEN_KEYBOARD` and the clipboard callbacks.
- Config: no default files on iOS. Write text to a file, `ghostty_config_load_file`.
  `setenv("GHOSTTY_RESOURCES_DIR", <bundle>/ghostty)` before `ghostty_init` so
  `themes/<name>` resolve.
- Keys: `ghostty_input_key_s.keycode` is a macOS virtual keycode (Enter 0x24,
  Backspace 0x33; table in ghostty `src/input/keycodes.zig`). Printable text goes via
  `ghostty_surface_text`, which takes the paste path and so honours bracketed paste.
  `UIKeyInput.insertText("\n")` must become an Enter key event.
- Logs: subsystem `com.mitchellh.ghostty`;
  `xcrun simctl spawn <udid> log show --predicate 'subsystem CONTAINS "ghostty"' --info`.
- Simulator: keyboard forced to `en_US@sw=QWERTY;hw=US` because `hw=Automatic` follows
  the host's Russian layout and `axe type` produced Cyrillic.

## Phases 4 and 5

- `ghostty_surface_free_text` is declared in `ghostty.h` with two parameters but exported
  with one. The bridging header aliases the real symbol as `sesh_ghostty_free_text`.
- `ghostty_surface_mouse_pos` takes points, not pixels: pass the view coordinate
  untouched, never multiplied by the content scale.
- There is no font-size entry point. `ghostty_surface_binding_action(surface,
  "set_font_size:12.0", len)` is how pinch-to-zoom changes one surface's size.
- `GHOSTTY_ACTION_SET_TITLE` arrives with `target.tag == GHOSTTY_TARGET_SURFACE`;
  `ghostty_surface_userdata` gives back the pointer passed as
  `ghostty_surface_config_s.userdata`, which is how the action finds the view. The title
  pointer is borrowed, so copy it, and the action runs off the main thread.
- `ghostty_surface_set_occlusion(surface, visible)` takes visibility, not occlusion. A
  Tab that is not on screen sets it false: the renderer stands down while
  `ghostty_surface_process_output` keeps feeding the VT.
- A `UIKeyInput` view gets iOS smart punctuation unless it says otherwise. Set
  `smartQuotesType`, `smartDashesType`, `smartInsertDeleteType`, `autocorrectionType`,
  `spellCheckingType` and `autocapitalizationType` off, or `printf '\033]0;x\007'` reaches
  the shell with typographic quotes.
