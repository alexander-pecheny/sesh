# libghostty-vt patches

`build.rs` copies `vendor/ghostty` (upstream ghostty at 44f2a44df7e8c4a0c6df3f7d872ef3d7ead88e51)
into the build directory and applies these patches in name order with `patch -p3`, so the
submodule stays pristine. Drop a patch once upstream offers the same API.

- `0002-expose-modify-other-keys-mode.patch` adds a terminal data query for xterm
  modifyOtherKeys mode 2, so an attached terminal can be asked for the key reports a pane wants.
- `0004-fix-hosted-wuffs-builds.patch` limits the Wuffs allocator stubs to freestanding targets;
  without it, builds with SIMD disabled replace the program's allocator.
- `0005-bounded-word-selection.patch` adds a word selection with a scan budget, used to select
  a wrapped word on double click without scanning an unbounded line.
- `0006-clear-screen-preserving-cursor-line.patch` adds a screen and history clear that keeps
  the line under the cursor.
